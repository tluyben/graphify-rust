BIN      := graphify
OUT      := dist
UNAME    := $(shell uname -s)

LINUX_TARGETS := \
	x86_64-unknown-linux-musl \
	aarch64-unknown-linux-musl

WINDOWS_TARGETS := \
	x86_64-pc-windows-gnu
# aarch64-pc-windows-gnullvm: no cross-rs Docker image exists yet; build on Windows with MSVC

MAC_TARGETS := \
	x86_64-apple-darwin \
	aarch64-apple-darwin

ifeq ($(UNAME),Darwin)
ALL_TARGETS := $(LINUX_TARGETS) $(WINDOWS_TARGETS) $(MAC_TARGETS)
else
ALL_TARGETS := $(LINUX_TARGETS) $(WINDOWS_TARGETS)
endif

.PHONY: all clean $(ALL_TARGETS)

all: $(ALL_TARGETS)
	@echo "Done. Binaries in $(OUT)/"

$(ALL_TARGETS):
	cross build --release --target $@
	@mkdir -p $(OUT)
	@if echo "$@" | grep -q 'windows'; then \
		cp target/$@/release/$(BIN).exe $(OUT)/$(BIN)-$@.exe; \
	else \
		cp target/$@/release/$(BIN) $(OUT)/$(BIN)-$@; \
	fi
	@echo "  -> $(OUT)/$(BIN)-$@"

clean:
	rm -rf $(OUT)
