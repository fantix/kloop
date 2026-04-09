// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2026 Fantix King

use std::{
    borrow::Cow,
    cell::{self, RefCell},
    fmt, io, iter,
    rc::Rc,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use compio::{
    buf::buf_try,
    driver::RawFd,
    io::{AsyncRead, AsyncWrite},
    rustls::{
        self, ALL_VERSIONS, ClientConfig, DigitallySignedStruct, ProtocolVersion, RootCertStore,
        ServerConfig, SignatureScheme, SupportedProtocolVersion,
        client::{
            WebPkiServerVerifier,
            danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        },
        crypto::{self, CryptoProvider, ring},
        pki_types::{
            CertificateDer, CertificateRevocationListDer, PrivateKeyDer, ServerName, UnixTime,
            pem::{PemObject, SectionKind},
        },
        server::{ClientHello, ResolvesServerCert, ResolvesServerCertUsingSni},
        sign::CertifiedKey,
        time_provider::{DefaultTimeProvider, TimeProvider},
    },
    tls::TlsStream,
};
use pyo3::{
    IntoPyObjectExt,
    exceptions::{PyKeyError, PyOSError, PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyTuple},
};
use webpki::{
    CertRevocationList, ExpirationPolicy, OwnedCertRevocationList, RevocationCheckDepth,
    UnknownStatusPolicy,
};

use crate::{Either, event_loop::CompioLoop, import, py_any_to_buffer, socket::SocketStream};

#[derive(Debug, Clone, Copy, Default)]
pub enum SSLImpl {
    #[default]
    OpenSSL,
    Rustls,
}

#[derive(Debug, Clone, Default)]
pub struct SSLSocketMetadata {
    pub implementation: SSLImpl,
    pub server_side: bool,
    pub fd: RawFd,
}

#[pyclass(unsendable)]
pub struct SSLSocket {
    pyloop: Py<CompioLoop>,
    inner: SSLSocketInner,
    metadata: SSLSocketMetadata,
}

impl SSLSocket {
    pub fn new(
        py: Python,
        pyloop: &Py<CompioLoop>,
        stream: TlsStream<SocketStream>,
        metadata: SSLSocketMetadata,
    ) -> PyResult<Py<Self>> {
        Py::new(
            py,
            Self {
                pyloop: pyloop.clone_ref(py),
                inner: SSLSocketInner::new(stream),
                metadata,
            },
        )
    }
}

#[pymethods]
impl SSLSocket {
    #[pyo3(signature = (bufsize, /))]
    fn recv<'py>(&self, py: Python<'py>, bufsize: usize) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let mut inner = inner.borrow_mut()?;
            let buf = Vec::with_capacity(bufsize);
            let (bytes_read, buf) = buf_try!(@try inner.read(buf).await);
            Python::attach(|py| {
                PyBytes::new_with_writer(py, bytes_read, |w| Ok(w.write_all(&buf[..bytes_read])?))?
                    .into_py_any(py)
            })
        })
    }

    #[pyo3(signature = (data, /))]
    fn send<'py>(&mut self, py: Python<'py>, data: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let mut inner = inner.borrow_mut()?;
            let buf = Python::attach(|py| py_any_to_buffer(py, data.bind(py)))?;
            let (bytes_written, _) = buf_try!(@try inner.write(buf).await);
            drop(data);
            inner.flush().await?;
            Python::attach(|py| bytes_written.into_py_any(py))
        })
    }

    fn close<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            if let Some(mut inner) = inner.borrow_mut_opt()?.take()
                && let Err(e) = inner.shutdown().await
                && e.kind() != io::ErrorKind::WouldBlock
            {
                Err(e)?;
            }
            Python::attach(|py| py.None().into_py_any(py))
        })
    }

    fn negotiated_alpn(&self) -> PyResult<Option<Vec<u8>>> {
        Ok(self.inner.borrow()?.negotiated_alpn().map(|x| x.to_vec()))
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(match self.inner.borrow_opt()?.as_ref() {
            Some(_) => format!(
                "<compio.SSLSocket impl={:?}, server_side={}, fd={:?}>",
                self.metadata.implementation, self.metadata.server_side, self.metadata.fd,
            ),
            None => "<compio.SSLSocket (closed)>".to_string(),
        })
    }
}

#[derive(Clone)]
struct SSLSocketInner(Rc<RefCell<Option<TlsStream<SocketStream>>>>);

impl SSLSocketInner {
    fn new(stream: TlsStream<SocketStream>) -> Self {
        Self(Rc::new(RefCell::new(Some(stream))))
    }

    #[inline]
    fn borrow_opt(&self) -> PyResult<cell::Ref<'_, Option<TlsStream<SocketStream>>>> {
        self.0
            .try_borrow()
            .map_err(|_| PyRuntimeError::new_err("concurrent access to SSLSocket"))
    }

    #[inline]
    fn borrow(&self) -> PyResult<cell::Ref<'_, TlsStream<SocketStream>>> {
        cell::Ref::filter_map(self.borrow_opt()?, |rv| rv.as_ref())
            .map_err(|_| PyOSError::new_err("socket is closed"))
    }

    #[inline]
    fn borrow_mut_opt(&self) -> PyResult<cell::RefMut<'_, Option<TlsStream<SocketStream>>>> {
        self.0
            .try_borrow_mut()
            .map_err(|_| PyRuntimeError::new_err("concurrent access to SSLSocket"))
    }

    #[inline]
    fn borrow_mut(&self) -> PyResult<cell::RefMut<'_, TlsStream<SocketStream>>> {
        cell::RefMut::filter_map(self.borrow_mut_opt()?, |rv| rv.as_mut())
            .map_err(|_| PyOSError::new_err("socket is closed"))
    }
}

#[pyclass]
pub struct RustlsContext {
    state: ContextState,
    protocol_versions: Py<RustlsProtocolVersions>,
    root_cert_store: Py<RustlsRootCertStore>,
    revocation_options: Py<RustlsRevocationOptions>,
}

#[pymethods]
impl RustlsContext {
    #[new]
    fn new(py: Python) -> PyResult<Self> {
        let state = ContextState::new()?;
        Ok(Self {
            protocol_versions: Py::new(py, RustlsProtocolVersions(state.clone()))?,
            root_cert_store: Py::new(py, RustlsRootCertStore(state.clone()))?,
            revocation_options: Py::new(py, RustlsRevocationOptions(state.clone()))?,
            state,
        })
    }

    #[getter]
    fn protocol_versions(&self) -> &Py<RustlsProtocolVersions> {
        &self.protocol_versions
    }

    #[setter]
    fn set_protocol_versions(&mut self, py: Python, versions: Bound<PyAny>) -> PyResult<()> {
        let mut state = self.state.write()?;

        // Preserve the old state so that we can leave the old RustlsProtocolVersions
        // object unchanged
        let old_state = RustlsContextState {
            provider: state.provider.clone(),
            time_provider: state.time_provider.clone(),
            protocol_versions: state.protocol_versions.clone(),
            alpn_protocols: None,
            root_cert_store: Arc::new(RootCertStore::empty()),
            custom_cert_verifier: None,
            revocation_options: Default::default(),
            server_cert: None,
            server_cert_resolver: Arc::new(ResolvesServerCertUsingSni::new()),
            config: None,
        };

        // Update the state with the new protocol versions
        let versions = versions
            .try_iter()?
            .map(|v| v.and_then(py_any_to_proto_ver))
            .collect::<PyResult<_>>()?;
        state.set_protocol_versions(versions)?;

        // Restore the old state into the old RustlsProtocolVersions object before
        // unlocking, in case it's still being referenced elsewhere
        self.protocol_versions.borrow_mut(py).0 = ContextState(Arc::new(RwLock::new(old_state)));
        drop(state);

        // Create a new RustlsProtocolVersions object pointing to the current state
        self.protocol_versions = Py::new(py, RustlsProtocolVersions(self.state.clone()))?;
        Ok(())
    }

    #[getter]
    fn alpn_protocols<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyTuple>>> {
        Ok(self
            .state
            .read()?
            .alpn_protocols
            .as_ref()
            .map(|l| l.bind(py).clone()))
    }

    #[setter]
    fn set_alpn_protocols(&self, py: Python, protocols: Option<Py<PyTuple>>) -> PyResult<()> {
        if let Some(protocols) = protocols.as_ref() {
            for proto in protocols.bind(py) {
                proto.cast::<PyBytes>()?;
            }
        }
        self.state.write()?.alpn_protocols = protocols;
        Ok(())
    }

    #[deleter]
    fn delete_alpn_protocols(&self) -> PyResult<()> {
        self.state.write()?.alpn_protocols = None;
        Ok(())
    }

    #[getter]
    fn root_cert_store(&self) -> &Py<RustlsRootCertStore> {
        &self.root_cert_store
    }

    #[getter]
    fn custom_server_certificate_verifier(&self, py: Python) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .state
            .read()?
            .custom_cert_verifier
            .as_ref()
            .map(|v| v.pyobj.clone_ref(py)))
    }

    #[setter]
    fn set_custom_server_certificate_verifier(&self, verifier: Option<Py<PyAny>>) -> PyResult<()> {
        self.state
            .write()?
            .set_custom_cert_verifier(verifier.map(|pyobj| CustomCertVerifier {
                state: self.state.clone(),
                pyobj,
            }))
    }

    #[deleter]
    fn delete_custom_server_certificate_verifier(&self) -> PyResult<()> {
        self.state.write()?.set_custom_cert_verifier(None)
    }

    #[getter]
    fn revocation_options(&self) -> &Py<RustlsRevocationOptions> {
        &self.revocation_options
    }

    #[pyo3(signature = (certificate_data, *, private_key=None, password=None, for_dns_name=None))]
    fn add_server_certificate_pem<'py>(
        &self,
        certificate_data: &str,
        private_key: Option<&str>,
        password: Option<Bound<PyAny>>,
        for_dns_name: Option<&str>,
    ) -> PyResult<()> {
        let mut pems =
            pem::parse_many(certificate_data).map_err(|e| PyValueError::new_err(e.to_string()))?;
        if let Some(private_key) = private_key {
            pems.push(pem::parse(private_key).map_err(|e| PyValueError::new_err(e.to_string()))?);
        }
        let password = password
            .as_ref()
            .map(|p| {
                if let Ok(p) = p.extract::<&str>() {
                    Ok(p.as_bytes())
                } else if let Ok(p) = p.extract::<&[u8]>() {
                    Ok(p)
                } else {
                    Err(PyValueError::new_err(
                        "password must be a string or bytes-like object",
                    ))
                }
            })
            .transpose()?;

        let mut certs = Vec::new();
        let mut key: Option<PrivateKeyDer> = None;
        for pem in pems {
            match pem.tag().as_bytes().try_into() {
                Ok(kind @ SectionKind::Certificate) => {
                    certs.push(CertificateDer::from_pem(kind, pem.into_contents()).expect("kind"));
                }
                Ok(
                    kind @ (SectionKind::PrivateKey
                    | SectionKind::RsaPrivateKey
                    | SectionKind::EcPrivateKey),
                ) => {
                    if key.is_some() {
                        return Err(PyValueError::new_err(
                            "multiple private keys found (only one allowed)",
                        ));
                    }
                    key = PrivateKeyDer::from_pem(kind, pem.into_contents());
                }
                Err(()) if pem.tag() == "ENCRYPTED PRIVATE KEY" => {
                    if key.is_some() {
                        return Err(PyValueError::new_err(
                            "multiple private keys found (only one allowed)",
                        ));
                    }
                    let Some(password) = password else {
                        return Err(PyValueError::new_err(
                            "encrypted private key found but no password provided",
                        ));
                    };
                    let info = pkcs8::EncryptedPrivateKeyInfo::try_from(pem.contents())
                        .map_err(|e| PyValueError::new_err(e.to_string()))?;
                    let doc = info
                        .decrypt(password)
                        .map_err(|e| PyValueError::new_err(e.to_string()))?;
                    let der = doc.to_bytes().to_vec();
                    key = Some(
                        PrivateKeyDer::try_from(der)
                            .map_err(|e| PyValueError::new_err(e.to_string()))?,
                    );
                }
                _ => {
                    let tag = pem.tag();
                    return Err(PyValueError::new_err(format!(
                        "unexpected PEM section: {tag}"
                    )));
                }
            }
        }
        let key = key.ok_or_else(|| PyValueError::new_err("no private key found"))?;
        match for_dns_name {
            Some(name) => self.state.write()?.add_server_cert(name, certs, key),
            None => self.state.write()?.set_server_cert(certs, key),
        }
    }
}

impl RustlsContext {
    pub(crate) fn build(
        &self,
        py: Python,
        server_side: bool,
    ) -> PyResult<Either<Arc<ServerConfig>, Arc<ClientConfig>>> {
        let mut state = self.state.write()?;
        match &state.config {
            Some(Either::Left(c)) if server_side => return Ok(Either::Left(c.clone())),
            Some(Either::Right(c)) if !server_side => return Ok(Either::Right(c.clone())),
            Some(_) => Err(PyRuntimeError::new_err(
                "cannot use one RustlsContext for both client and server",
            ))?,
            None => {}
        }
        if let Some(config) = &state.config {
            return Ok(config.clone());
        }
        let versions = supported_protocol_versions(&state.protocol_versions)?;
        let alpn_protocols = state
            .alpn_protocols
            .as_ref()
            .map(|protos| {
                protos
                    .bind(py)
                    .iter()
                    .map(|p| Ok(p.cast::<PyBytes>()?.as_bytes().to_vec()))
                    .collect::<PyResult<Vec<_>>>()
            })
            .transpose()?;
        let rv = if server_side {
            let mut config = ServerConfig::builder_with_details(
                state.provider.clone(),
                state.time_provider.clone(),
            )
            .with_protocol_versions(&versions)
            .expect("checked")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(ServerCertResolver {
                fallback: state.server_cert.take(),
                resolver: state.server_cert_resolver.clone(),
            }));
            if let Some(protocols) = alpn_protocols {
                config.alpn_protocols = protocols;
            }
            Either::Left(Arc::new(config))
        } else {
            let builder = ClientConfig::builder_with_details(
                state.provider.clone(),
                state.time_provider.clone(),
            )
            .with_protocol_versions(&versions)
            .expect("checked");
            let builder = if let Some(verifier) = &state.custom_cert_verifier {
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(verifier.clone())
            } else {
                let mut verifier_builder = WebPkiServerVerifier::builder_with_provider(
                    state.root_cert_store.clone(),
                    state.provider.clone(),
                )
                .with_crls(state.revocation_options.crls.iter().map(|(_, b)| b.clone()));
                if state.revocation_options.only_check_end_entity_revocation {
                    verifier_builder = verifier_builder.only_check_end_entity_revocation();
                }
                if state.revocation_options.allow_unknown_revocation_status {
                    verifier_builder = verifier_builder.allow_unknown_revocation_status();
                }
                if state.revocation_options.enforce_revocation_expiration {
                    verifier_builder = verifier_builder.enforce_revocation_expiration();
                }
                match verifier_builder.build() {
                    Ok(verifier) => builder.with_webpki_verifier(verifier),
                    Err(e) => Err(PyValueError::new_err(e.to_string()))?,
                }
            };
            let mut config = builder.with_no_client_auth();
            if let Some(protocols) = alpn_protocols {
                config.alpn_protocols = protocols;
            }
            Either::Right(Arc::new(config))
        };
        state.config = Some(rv.clone());
        Ok(rv)
    }
}

struct RustlsContextState {
    provider: Arc<CryptoProvider>,
    time_provider: Arc<dyn TimeProvider>,
    protocol_versions: Vec<ProtocolVersion>,
    alpn_protocols: Option<Py<PyTuple>>,
    root_cert_store: Arc<RootCertStore>,
    custom_cert_verifier: Option<Arc<CustomCertVerifier>>,
    revocation_options: MutRevocationOptions,
    server_cert: Option<Arc<CertifiedKey>>,
    server_cert_resolver: Arc<ResolvesServerCertUsingSni>,
    config: Option<Either<Arc<ServerConfig>, Arc<ClientConfig>>>,
}

impl RustlsContextState {
    fn new() -> PyResult<Self> {
        Ok(Self {
            provider: match CryptoProvider::get_default() {
                Some(p) => p.clone(),
                None => {
                    ring::default_provider().install_default().ok();
                    CryptoProvider::get_default().expect("default").clone()
                }
            },
            time_provider: Arc::new(DefaultTimeProvider),
            protocol_versions: ALL_VERSIONS.iter().map(|v| v.version).collect(),
            alpn_protocols: None,
            root_cert_store: Arc::new(RootCertStore::empty()),
            custom_cert_verifier: None,
            revocation_options: MutRevocationOptions::default(),
            server_cert: None,
            server_cert_resolver: Arc::new(ResolvesServerCertUsingSni::new()),
            config: None,
        })
    }

    fn set_protocol_versions(&mut self, versions: Vec<ProtocolVersion>) -> PyResult<()> {
        if versions.is_empty() {
            return Err(PyValueError::new_err(
                "at least one protocol version must be enabled",
            ));
        }
        let supported_versions = supported_protocol_versions(&versions)?;
        ServerConfig::builder_with_details(self.provider.clone(), self.time_provider.clone())
            .with_protocol_versions(&supported_versions)
            .and_then(|_| {
                ClientConfig::builder_with_details(
                    self.provider.clone(),
                    self.time_provider.clone(),
                )
                .with_protocol_versions(&supported_versions)
            })
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        self.protocol_versions = versions;
        Ok(())
    }

    fn certify_key(
        &self,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> PyResult<CertifiedKey> {
        CertifiedKey::from_der(certs, key, &self.provider)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn add_server_cert(
        &mut self,
        dns_name: &str,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> PyResult<()> {
        let certified_key = self.certify_key(certs, key)?;
        Arc::get_mut(&mut self.server_cert_resolver)
            .expect("unused")
            .add(dns_name, certified_key)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn set_server_cert(
        &mut self,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> PyResult<()> {
        let certified_key = self.certify_key(certs, key)?;
        self.server_cert = Some(Arc::new(certified_key));
        Ok(())
    }

    fn add_root_cert(&mut self, der: CertificateDer) -> PyResult<()> {
        Arc::get_mut(&mut self.root_cert_store)
            .ok_or_else(|| PyRuntimeError::new_err("RustlsContext is already in use"))?
            .add(der)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn set_custom_cert_verifier(&mut self, verifier: Option<CustomCertVerifier>) -> PyResult<()> {
        match verifier {
            Some(verifier) => {
                self.custom_cert_verifier = Some(Arc::new(verifier));
            }
            None => self.custom_cert_verifier = None,
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ContextState(Arc<RwLock<RustlsContextState>>);

impl ContextState {
    fn new() -> PyResult<Self> {
        let state = RustlsContextState::new()?;
        Ok(Self(Arc::new(RwLock::new(state))))
    }

    #[inline]
    fn read(&'_ self) -> PyResult<RwLockReadGuard<'_, RustlsContextState>> {
        self.0
            .read()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    #[inline]
    fn write(&'_ self) -> PyResult<RwLockWriteGuard<'_, RustlsContextState>> {
        let rv = self
            .0
            .write()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        if rv.config.is_some() {
            return Err(PyRuntimeError::new_err(
                "cannot modify RustlsContext after being used",
            ));
        }
        Ok(rv)
    }

    #[inline]
    fn with_provider<T, F>(&self, f: F) -> Result<T, rustls::Error>
    where
        F: FnOnce(&CryptoProvider) -> Result<T, rustls::Error>,
    {
        f(self
            .0
            .read()
            .map_err(|e| rustls::Error::General(e.to_string()))?
            .provider
            .as_ref())
    }
}

#[derive(Debug)]
struct ServerCertResolver {
    resolver: Arc<ResolvesServerCertUsingSni>,
    fallback: Option<Arc<CertifiedKey>>,
}

impl ResolvesServerCert for ServerCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.resolver
            .resolve(client_hello)
            .or_else(|| self.fallback.clone())
    }
}

#[pyclass]
struct RustlsProtocolVersions(ContextState);

#[pymethods]
impl RustlsProtocolVersions {
    fn add(&self, version: &Bound<PyAny>) -> PyResult<()> {
        let version = py_any_to_proto_ver(version)?;
        let mut state = self.0.write()?;
        if !state.protocol_versions.contains(&version) {
            let versions = iter::once(version)
                .chain(state.protocol_versions.iter().copied())
                .collect();
            state.set_protocol_versions(versions)?;
        }
        Ok(())
    }

    fn remove(&self, version: Bound<PyAny>) -> PyResult<()> {
        self.remove_ex(version, true)
    }

    fn discard(&self, version: Bound<PyAny>) -> PyResult<()> {
        self.remove_ex(version, false)
    }

    fn __len__(&self) -> PyResult<usize> {
        Ok(self.0.read()?.protocol_versions.len())
    }

    fn __contains__(&self, version: &Bound<PyAny>) -> PyResult<bool> {
        let version = py_any_to_proto_ver(version)?;
        Ok(self.0.read()?.protocol_versions.contains(&version))
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let versions = self
            .0
            .read()?
            .protocol_versions
            .iter()
            .map(|v| import::ssl::tls_version(py, (*v).into()))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, versions)?.call_method0("__iter__")
    }

    fn __repr__(&self, py: Python) -> PyResult<String> {
        let versions = self
            .0
            .read()?
            .protocol_versions
            .iter()
            .map(|v| import::ssl::tls_version(py, (*v).into()).map(|v| format!("{v:?}")))
            .collect::<PyResult<Vec<_>>>()?
            .join(", ");
        Ok(format!("{{{versions}}}"))
    }
}

impl RustlsProtocolVersions {
    fn remove_ex(&self, version: Bound<PyAny>, raise: bool) -> PyResult<()> {
        let ver = py_any_to_proto_ver(&version)?;
        let mut dropped = false;
        let mut state = self.0.write()?;
        let versions = state
            .protocol_versions
            .iter()
            .copied()
            .filter(|v| {
                if *v == ver {
                    dropped = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        if dropped {
            state.set_protocol_versions(versions)?;
        } else if raise {
            return Err(PyKeyError::new_err(version.unbind()));
        }
        Ok(())
    }
}

#[pyclass]
struct RustlsRootCertStore(ContextState);

#[pymethods]
impl RustlsRootCertStore {
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.0.read()?.root_cert_store.len())
    }

    fn add_der(&self, py: Python, cert: Bound<PyAny>) -> PyResult<()> {
        let cert = py_any_to_buffer(py, &cert)?;
        self.0
            .write()?
            .add_root_cert(CertificateDer::from(cert.as_ref()))
    }

    fn add_pem(&self, cert: &str) -> PyResult<()> {
        let mut state = self.0.write()?;
        for der in CertificateDer::pem_slice_iter(cert.as_bytes()) {
            let der = der.map_err(|e| PyValueError::new_err(e.to_string()))?;
            state.add_root_cert(der)?;
        }
        Ok(())
    }
}

struct CustomCertVerifier {
    state: ContextState,
    pyobj: Py<PyAny>,
}

impl fmt::Debug for CustomCertVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomCertVerifier")
            .field("pyobj", &format_args!("{:?}", self.pyobj))
            .finish()
    }
}

impl ServerCertVerifier for CustomCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match Python::attach(|py| {
            let cert = RustlsServerCertificate::new(self.state.clone(), end_entity)?;
            self.pyobj.call_method1(
                py,
                "verify_server_cert",
                (
                    cert,
                    intermediates.iter().map(|c| c.as_ref()).collect::<Vec<_>>(),
                    RustlsServerName(server_name.to_owned()),
                    ocsp_response,
                    RustlsUnixTime(now),
                ),
            )
        }) {
            Ok(_) => Ok(ServerCertVerified::assertion()),
            Err(e) => Err(rustls::Error::Other(rustls::OtherError(Arc::new(e)))),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.state.with_provider(|provider| {
            crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &provider.signature_verification_algorithms,
            )
        })
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.state.with_provider(|provider| {
            crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &provider.signature_verification_algorithms,
            )
        })
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.state
            .with_provider(|provider| {
                Ok(provider
                    .signature_verification_algorithms
                    .supported_schemes())
            })
            .expect("not poisoned")
    }
}

#[pyclass]
struct RustlsServerCertificate {
    state: ContextState,
    // Store the owned certificate data to ensure 'static lifetime
    _cert_der: CertificateDer<'static>,
    end_entity: webpki::EndEntityCert<'static>,
}

#[pymethods]
impl RustlsServerCertificate {
    fn verify_trust_chain(
        &self,
        intermediate_certs: Bound<PyAny>,
        time: Bound<RustlsUnixTime>,
    ) -> PyResult<()> {
        let state = self.state.read()?;
        let crls = state
            .revocation_options
            .crls
            .iter()
            .map(|(c, _)| c)
            .collect::<Vec<_>>();
        let revocation = webpki::RevocationOptionsBuilder::new(crls.as_slice())
            .ok()
            .map(|builder| {
                builder
                    .with_depth(
                        if state.revocation_options.only_check_end_entity_revocation {
                            RevocationCheckDepth::EndEntity
                        } else {
                            RevocationCheckDepth::Chain
                        },
                    )
                    .with_status_policy(
                        if state.revocation_options.allow_unknown_revocation_status {
                            UnknownStatusPolicy::Deny
                        } else {
                            UnknownStatusPolicy::Allow
                        },
                    )
                    .with_expiration_policy(
                        if state.revocation_options.enforce_revocation_expiration {
                            ExpirationPolicy::Enforce
                        } else {
                            ExpirationPolicy::Ignore
                        },
                    )
                    .build()
            });
        let intermediate_certs = intermediate_certs
            .try_iter()?
            .collect::<PyResult<Vec<_>>>()?;
        let intermediate_certs = intermediate_certs
            .iter()
            .map(|c| Ok(CertificateDer::from(c.extract::<&[u8]>()?)))
            .collect::<PyResult<Vec<_>>>()?;
        let res = self.end_entity.verify_for_usage(
            state.provider.signature_verification_algorithms.all,
            &state.root_cert_store.roots,
            &intermediate_certs,
            time.borrow().0,
            webpki::KeyUsage::server_auth(),
            revocation,
            None,
        );
        match res {
            Ok(_) => Ok(()),
            Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
        }
    }

    fn verify_server_name(&self, server_name: Bound<RustlsServerName>) -> PyResult<()> {
        self.end_entity
            .verify_is_valid_for_subject_name(&server_name.borrow().0)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }
}

impl RustlsServerCertificate {
    fn new(state: ContextState, end_entity: &CertificateDer<'_>) -> PyResult<Self> {
        // Clone the certificate to get owned data with 'static lifetime
        let cert_der: CertificateDer<'static> = end_entity.clone().into_owned();

        // SAFETY: We're extending the lifetime of the reference to 'static.
        // This is safe because:
        // 1. cert_der is owned by this struct and will live as long as the struct
        // 2. The EndEntityCert only borrows from cert_der
        // 3. Both are stored in the same struct, ensuring the borrow is valid
        let cert_ref: &'static CertificateDer<'static> = unsafe { std::mem::transmute(&cert_der) };

        Ok(Self {
            state,
            _cert_der: cert_der,
            end_entity: webpki::EndEntityCert::try_from(cert_ref)
                .map_err(|e| PyValueError::new_err(format!("{e:?}")))?,
        })
    }
}

#[pyclass]
struct RustlsServerName(ServerName<'static>);

#[pymethods]
impl RustlsServerName {
    fn __str__(&self) -> Cow<'_, str> {
        self.0.to_str()
    }

    fn __repr__(&self) -> String {
        match &self.0 {
            ServerName::DnsName(dns) => format!("{dns:?}"),
            ServerName::IpAddress(ip) => format!("{ip:?}"),
            _ => unimplemented!(),
        }
    }

    fn is_ip_address(&self) -> bool {
        matches!(self.0, ServerName::IpAddress(_))
    }

    fn is_dns_name(&self) -> bool {
        matches!(self.0, ServerName::DnsName(_))
    }
}

#[pyclass]
struct RustlsUnixTime(UnixTime);

#[pymethods]
impl RustlsUnixTime {
    #[new]
    fn new() -> Self {
        Self(UnixTime::now())
    }

    fn __int__(&self) -> u64 {
        self.0.as_secs()
    }
}

#[derive(Default)]
struct MutRevocationOptions {
    crls: Vec<(
        CertRevocationList<'static>,
        CertificateRevocationListDer<'static>,
    )>,
    only_check_end_entity_revocation: bool,
    allow_unknown_revocation_status: bool,
    enforce_revocation_expiration: bool,
}

impl MutRevocationOptions {
    fn add_crl_der(&mut self, der: impl AsRef<[u8]>) -> PyResult<()> {
        let der = der.as_ref().to_owned();
        OwnedCertRevocationList::from_der(&der)
            .map(|crl| self.crls.push((crl.into(), der.into())))
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

#[pyclass]
struct RustlsRevocationOptions(ContextState);

#[pymethods]
impl RustlsRevocationOptions {
    fn add_crl_der(&self, py: Python, der: Bound<PyAny>) -> PyResult<()> {
        let der = py_any_to_buffer(py, &der)?;
        self.0.write()?.revocation_options.add_crl_der(der)
    }

    fn crls_count(&self) -> PyResult<usize> {
        Ok(self.0.read()?.revocation_options.crls.len())
    }

    #[getter]
    fn only_check_end_entity_revocation(&self) -> PyResult<bool> {
        Ok(self
            .0
            .read()?
            .revocation_options
            .only_check_end_entity_revocation)
    }

    #[setter]
    fn set_only_check_end_entity_revocation(&self, value: bool) -> PyResult<()> {
        self.0
            .write()?
            .revocation_options
            .only_check_end_entity_revocation = value;
        Ok(())
    }

    #[getter]
    fn allow_unknown_revocation_status(&self) -> PyResult<bool> {
        Ok(self
            .0
            .read()?
            .revocation_options
            .allow_unknown_revocation_status)
    }

    #[setter]
    fn set_allow_unknown_revocation_status(&self, value: bool) -> PyResult<()> {
        self.0
            .write()?
            .revocation_options
            .allow_unknown_revocation_status = value;
        Ok(())
    }

    #[getter]
    fn enforce_revocation_expiration(&self) -> PyResult<bool> {
        Ok(self
            .0
            .read()?
            .revocation_options
            .enforce_revocation_expiration)
    }

    #[setter]
    fn set_enforce_revocation_expiration(&self, value: bool) -> PyResult<()> {
        self.0
            .write()?
            .revocation_options
            .enforce_revocation_expiration = value;
        Ok(())
    }
}

fn supported_protocol_versions(
    versions: &Vec<ProtocolVersion>,
) -> PyResult<Vec<&'static SupportedProtocolVersion>> {
    versions
        .iter()
        .copied()
        .map(|v| {
            ALL_VERSIONS
                .iter()
                .find_map(|av| (v == av.version).then_some(*av))
                .ok_or_else(|| {
                    PyValueError::new_err(format!("unsupported protocol version: {v:?}"))
                })
        })
        .collect()
}

fn py_any_to_proto_ver<'a>(version: impl AsRef<Bound<'a, PyAny>>) -> PyResult<ProtocolVersion> {
    let version: u16 = version.as_ref().getattr("value")?.extract()?;
    Ok(ProtocolVersion::from(version))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SSLSocket>()?;
    m.add_class::<RustlsContext>()?;
    m.add_class::<RustlsProtocolVersions>()?;
    m.add_class::<RustlsRootCertStore>()?;
    m.add_class::<RustlsServerCertificate>()?;
    m.add_class::<RustlsServerName>()?;
    m.add_class::<RustlsUnixTime>()?;
    m.add_class::<RustlsRevocationOptions>()?;
    Ok(())
}
