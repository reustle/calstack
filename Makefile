PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo
VERSION := 0.1.0

APP_NAME := Calstack
BUNDLE := dist/$(APP_NAME).app

.PHONY: all build check install uninstall dist clean bundle-macos install-macos uninstall-macos
all: build
build:
	$(CARGO) build --release --locked
check:
	$(CARGO) fmt --all -- --check
	$(CARGO) test --workspace --locked
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings
install: build
	install -Dm755 target/release/calstack "$(DESTDIR)$(PREFIX)/bin/calstack"
	install -Dm644 packaging/calstack.desktop "$(DESTDIR)$(PREFIX)/share/applications/calstack.desktop"
	install -Dm644 LICENSE "$(DESTDIR)$(PREFIX)/share/licenses/calstack/LICENSE"
uninstall:
	rm -f "$(DESTDIR)$(PREFIX)/bin/calstack"
	rm -f "$(DESTDIR)$(PREFIX)/share/applications/calstack.desktop"
	rm -f "$(DESTDIR)$(PREFIX)/share/licenses/calstack/LICENSE"
dist:
	mkdir -p dist
	tar --exclude=__pycache__ --transform 's,^,calstack-$(VERSION)/,' -czf "dist/calstack-$(VERSION).tar.gz" Cargo.toml Cargo.lock Makefile LICENSE README.md PLAN.md crates assets packaging docs scripts
	cp packaging/arch/PKGBUILD dist/PKGBUILD
	cd dist && sed -i "s/@SHA256@/$$(sha256sum calstack-$(VERSION).tar.gz | cut -d' ' -f1)/" PKGBUILD
clean:
	$(CARGO) clean

# macOS .app bundle. Not part of `dist`/`install` above, which target Linux
# packaging; `cargo build --release` (no --locked) since this is a local
# dev-machine workflow, not CI.
bundle-macos:
	$(CARGO) build --release
	rm -rf "$(BUNDLE)"
	mkdir -p "$(BUNDLE)/Contents/MacOS"
	cp target/release/calstack "$(BUNDLE)/Contents/MacOS/calstack"
	sed "s/@VERSION@/$(VERSION)/g" packaging/macos/Info.plist > "$(BUNDLE)/Contents/Info.plist"
	@echo "Built $(BUNDLE)"
install-macos: bundle-macos
	rm -rf "/Applications/$(APP_NAME).app"
	cp -R "$(BUNDLE)" "/Applications/$(APP_NAME).app"
	@echo "Installed to /Applications/$(APP_NAME).app"
uninstall-macos:
	rm -rf "/Applications/$(APP_NAME).app"
