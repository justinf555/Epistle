run:
	flatpak-builder --user --install --force-clean flatpak-build-dir io.github.justinf555.Epistle.json && \
	flatpak run io.github.justinf555.Epistle

debug:
	flatpak-builder --user --install --force-clean flatpak-build-dir io.github.justinf555.Epistle.json && \
	flatpak run --env=RUST_LOG=epistle=debug io.github.justinf555.Epistle

clean:
	rm -rf flatpak-build-dir

# ── Testing (inside GNOME 50 Flatpak SDK) ────────────────────────────────────
#
# All test targets run inside the Flatpak SDK so that GTK 4.22,
# libadwaita 1.9 and other GNOME 50 dependencies are available.

# Flatpak SDK runner — uses an isolated CARGO_HOME to avoid rustup shims
# in ~/.cargo/bin shadowing the SDK's toolchain. Registry and git caches
# are symlinked from the host for speed.
FLATPAK_RUN = flatpak run --share=network \
	--filesystem=$(CURDIR) \
	--filesystem=$(HOME)/.cargo/registry:create \
	--filesystem=$(HOME)/.cargo/git:create \
	--env=CARGO_HOME=/tmp/flatpak-cargo \
	--command=bash org.gnome.Sdk//50

# Preamble sourced before every SDK command — sets up toolchain and cargo home.
SDK_INIT = source /usr/lib/sdk/rust-stable/enable.sh && \
	mkdir -p /tmp/flatpak-cargo/bin && \
	ln -sf $(HOME)/.cargo/registry /tmp/flatpak-cargo/registry 2>/dev/null; \
	ln -sf $(HOME)/.cargo/git /tmp/flatpak-cargo/git 2>/dev/null; \
	export PATH=/tmp/flatpak-cargo/bin:$$PATH && \
	cd $(CURDIR)

check:
	$(FLATPAK_RUN) -c '$(SDK_INIT) && cargo check'

test:
	$(FLATPAK_RUN) -c '$(SDK_INIT) && cargo test'

# Integration tests — need session D-Bus for GOA, EDS, etc.
FLATPAK_RUN_DBUS = flatpak run --share=network \
	--socket=session-bus \
	--filesystem=$(CURDIR) \
	--filesystem=$(HOME)/.cargo/registry:create \
	--filesystem=$(HOME)/.cargo/git:create \
	--env=CARGO_HOME=/tmp/flatpak-cargo \
	--command=bash org.gnome.Sdk//50

integration-test:
	$(FLATPAK_RUN_DBUS) -c '$(SDK_INIT) && cargo test -- --ignored'

# ── Linting & Analysis ──────────────────────────────────────────────────────

lint:
	$(FLATPAK_RUN) -c '$(SDK_INIT) && cargo clippy --all-targets -- -D warnings'

fmt:
	$(FLATPAK_RUN) -c '$(SDK_INIT) && cargo fmt'

fmt-check:
	$(FLATPAK_RUN) -c '$(SDK_INIT) && cargo fmt -- --check'
