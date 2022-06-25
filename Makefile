.PHONY: dist dev build clean
.DEFAULT_GOAL := dev


# Make distribution tarballs
dist:
	pip install -U build
	python -m build


# Incrementally build for development
dev:
	KLOOP_DEBUG=1 python setup.py develop


# Always build for development
build:
	KLOOP_FORCE=1 KLOOP_DEBUG=1 python setup.py develop


# Clean up everything including the Rust build cache
clean:
	python setup.py clean --all
