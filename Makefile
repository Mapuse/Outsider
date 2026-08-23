include env.mk

PROFILE ?= release
TARGET  := target/$(RUST_TARGET)/$(PROFILE)/ous
DESTDIR  ?=

# ── cps: external package, fetched from upstream ─────────────────────────
CPS_URL ?= https://github.com/Mapuse/CPS
CPS_DIR ?= $(HOME)/cudane-deps/cps
# CPS_REV lives in env.mk — the single source of truth shared by every builder.
CPS_REF ?= $(CPS_REV)

$(CPS_DIR):
	git clone $(CPS_URL) $(CPS_DIR)
	git -C $(CPS_DIR) checkout $(CPS_REF)

.PHONY: all build deps cps-pkg install install-man install-cps clean uninstall

all: build

deps: $(CPS_DIR)

build: $(CPS_DIR)
	CARGO_TARGET_DIR=$(CURDIR)/target cargo build --target $(RUST_TARGET) --profile $(PROFILE) --locked

# Build the `cps` package (binary + themes/data) from the cloned upstream
# repo, so Outsider ships the Python subsystem alongside itself.
cps-pkg: $(CPS_DIR)
	CARGO_TARGET_DIR=$(CURDIR)/target/cps cargo build --release --locked --manifest-path $(CPS_DIR)/Cargo.toml

install: build install-man install-cps
	install -Dm755 $(TARGET) $(DESTDIR)$(PREFIX)/bin/ous

install-man:
	install -d $(DESTDIR)$(PREFIX)/share/man/man1
	install -m 644 docs/ous.1 $(DESTDIR)$(PREFIX)/share/man/man1/

install-cps: cps-pkg
	install -Dm755 $(CURDIR)/target/cps/release/cps $(DESTDIR)$(PREFIX)/bin/cps
	install -d $(DESTDIR)$(PREFIX)/share/cps/themes $(DESTDIR)$(PREFIX)/share/cps/examples
	if [ -d themes ]; then install -m644 themes/*.py $(DESTDIR)$(PREFIX)/share/cps/themes/; fi
	if [ -d "$(CPS_DIR)/themes" ]; then \
		install -m644 $(CPS_DIR)/themes/*.py $(DESTDIR)$(PREFIX)/share/cps/themes/; fi
	if [ -f "$(CPS_DIR)/t.desc" ]; then install -m644 $(CPS_DIR)/t.desc $(CPS_DIR)/p.desc $(DESTDIR)$(PREFIX)/share/cps/; fi
	if [ -d "$(CPS_DIR)/examples" ]; then install -m644 $(CPS_DIR)/examples/*.py $(DESTDIR)$(PREFIX)/share/cps/examples/; fi

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ous
	rm -f $(DESTDIR)$(PREFIX)/bin/cps
	rm -f $(DESTDIR)$(PREFIX)/share/man/man1/ous.1
	rm -rf $(DESTDIR)$(PREFIX)/share/cps

clean:
	cargo clean
	rm -rf target/cps
