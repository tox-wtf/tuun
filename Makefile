-include config.mk

all: tuun scripts

tuun:
	PREFIX=$(PREFIX) \
	BINDIR=$(BINDIR) \
	DATADIR=$(DATADIR) \
	LIBEXECDIR=$(LIBEXECDIR) \
	cargo build --release

SCRIPTS := $(shell find scripts -type f)
TARGET_SCRIPTS := $(patsubst scripts/%,target/scripts/%,$(SCRIPTS))

scripts: $(TARGET_SCRIPTS)

$(TARGET_SCRIPTS): target/scripts/% : scripts/%
	@mkdir -p $(dir $@)
	sed -e 's,%PREFIX%,$(PREFIX),g' \
	    -e 's,%BINDIR%,$(BINDIR),g' \
	    -e 's,%DATADIR%,$(DATADIR),g' \
	    -e 's,%LIBEXECDIR%,$(LIBEXECDIR),g' \
	    $< > $@

clean:
	cargo clean

lint:
	cargo clippy

fmt: format

format:
	cargo fmt

install:
	install -Dm644 ./config.toml            $(DESTDIR)$(DATADIR)/default_config.toml
	install -Dm755 target/release/tuun      $(DESTDIR)$(LIBEXECDIR)/tuun
	install -Dm755 target/scripts/tuun.sh   $(DESTDIR)$(BINDIR)/tuun
	install -Dm755 target/scripts/quu.sh    $(DESTDIR)$(BINDIR)/quu
	install -Dm755 target/scripts/fzm       $(DESTDIR)$(BINDIR)/fzm

uninstall:
	rm -rf $(DESTDIR)$(DATADIR)   $(DESTDIR)/tmp/tuun
	rm -f  $(DESTDIR)$(LIBEXECDIR)/tuun $(DESTDIR)$(BINDIR)/tuun $(DESTDIR)$(BINDIR)/quu $(DESTDIR)$(BINDIR)/fzm

.PHONY: scripts
