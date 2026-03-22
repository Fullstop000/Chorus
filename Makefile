.PHONY: build-ui build release hooks

hooks:
	git config core.hooksPath .hooks
	@echo "Git hooks installed."

build-ui:
	cd ui && npm install && npm run build

build: build-ui
	cargo build --release

release: build
	@echo "Build complete. Run: ./target/release/chorus serve"
