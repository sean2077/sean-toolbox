# Sean Toolbox development commands.
# Run from the repository root with a POSIX shell available (Git Bash on Windows).
# `make help` lists the available targets.

CARGO ?= cargo
PYTHON ?= python

ifeq ($(OS),Windows_NT)
EXE := .exe
else
EXE :=
endif

DEBUG_HOST := target/debug/sean-toolbox$(EXE)
RELEASE_HOST := target/release/sean-toolbox$(EXE)

.DEFAULT_GOAL := help
.PHONY: help fmt fmt-check clippy test check build build-release run run-release test-x11 smoke licenses package clean

help: ## Show this help
	@echo "Sean Toolbox development targets:"
	@echo ""
	@echo "  make fmt            Format Rust sources"
	@echo "  make fmt-check      Check Rust formatting"
	@echo "  make clippy         Run Clippy with warnings as errors"
	@echo "  make test           Run workspace unit and contract tests"
	@echo "  make check          fmt-check + clippy + test"
	@echo "  make build          Build debug host and UI binaries"
	@echo "  make build-release  Build release host and UI binaries"
	@echo "  make run            Build and run the debug host"
	@echo "  make run-release    Build and run the release host"
	@echo "  make test-x11       Run the Linux X11 real-window smoke test"
	@echo "  make smoke          Run the Linux host/UI lifecycle smoke test"
	@echo "  make licenses       Collect third-party license texts"
	@echo "  make package        Build, collect licenses, and create dist/ archives"
	@echo "  make clean          Remove Cargo build output"

fmt: ## Format Rust sources
	$(CARGO) fmt --all

fmt-check: ## Check Rust formatting
	$(CARGO) fmt --all -- --check

clippy: ## Lint all workspace targets with warnings as errors
	$(CARGO) clippy --locked --workspace --all-targets -- -D warnings

test: ## Run workspace unit and contract tests
	$(CARGO) test --locked --workspace

check: fmt-check clippy test ## Run the standard local gate

build: ## Build debug host and UI binaries
	$(CARGO) build --locked -p toolbox-host -p toolbox-ui

build-release: ## Build release host and UI binaries
	$(CARGO) build --locked --release -p toolbox-host -p toolbox-ui

run: build ## Build and run the debug host
	./$(DEBUG_HOST)

run-release: build-release ## Build and run the release host
	./$(RELEASE_HOST)

test-x11: ## Run the Linux X11 real-window smoke test
	bash crates/toolbox-windows/tests/run-x11-smoke.sh

smoke: build-release ## Run the Linux host/UI lifecycle smoke test
	$(PYTHON) scripts/smoke_host.py --binary $(RELEASE_HOST)

licenses: ## Collect third-party license texts into target/
	$(CARGO) fetch --locked
	$(PYTHON) scripts/collect_licenses.py

package: build-release ## Build, collect licenses, and create dist/ archives
	$(CARGO) fetch --locked
	$(PYTHON) scripts/collect_licenses.py
	$(PYTHON) scripts/package.py

clean: ## Remove Cargo build output
	$(CARGO) clean