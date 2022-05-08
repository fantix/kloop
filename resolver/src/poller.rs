use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use futures_util::task::noop_waker_ref;

use libc;

pub type BoxFuture<'a, T> = Box<dyn Future<Output = T> + 'a>;

pub struct Poller<'a> {
    ctx: Context<'a>,
    fut: BoxFuture<'a, libc::c_int>,
}

impl<'a> Poller<'a> {
    pub fn new(fut: impl Future<Output = libc::c_int> + 'a) -> Self {
        let waker = noop_waker_ref();
        Poller {
            ctx: Context::from_waker(waker),
            fut: Box::new(fut),
        }
    }

    pub fn step(&mut self) -> Option<libc::c_int> {
        let fut = unsafe { Pin::new_unchecked(self.fut.as_mut()) };
        match fut.poll(&mut self.ctx) {
            Poll::Ready(rv) => Some(rv),
            Poll::Pending => None,
        }
    }
}

pub struct OnceFuture {
    seen: bool,
}

impl OnceFuture {
    pub fn new() -> Self {
        Self { seen: false }
    }

}

impl Future for OnceFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.seen {
            println!("poll ready");
            Poll::Ready(())
        } else {
            println!("poll pending");
            self.get_mut().seen = true;
            Poll::Pending
        }
    }
}
