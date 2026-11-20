.DEFAULT_GOAL := build

# Hardware targets share the same ST-LINK connection.
.NOTPARALLEL:

# Firmware and application builds
CARGO ?= cargo
CARGO_TARGET_DIR ?= target
CARGO_FLAGS ?=
PROFILE ?= debug

TARGET := thumbv7em-none-eabihf
CARGO_PROFILE = $(if $(filter debug,$(PROFILE)),dev,$(PROFILE))
ARTIFACT_DIR := $(CARGO_TARGET_DIR)/$(TARGET)/$(PROFILE)
KERNEL_ELF := $(ARTIFACT_DIR)/kr-stm-os
BUILD_ARGS = --locked --target $(TARGET) --target-dir "$(CARGO_TARGET_DIR)" \
	--profile $(CARGO_PROFILE) $(CARGO_FLAGS)

.PHONY: build build-apps
build:
	$(CARGO) build $(BUILD_ARGS) -p kr-stm-os

build-apps:
	$(CARGO) build $(BUILD_ARGS) -p user_lib --bins

# Application images
PYTHON ?= python3
# Cargo uses the file stem as the binary name for apps in user/src/bin.
APPS ?= $(sort $(basename $(notdir $(wildcard user/src/bin/*.rs))))
OUTPUT_DIR ?= output
QSPI_IMAGE ?= $(OUTPUT_DIR)/apps.img

.PHONY: pack-apps
pack-apps: build-apps
	$(PYTHON) tools/pack-apps -o "$(QSPI_IMAGE)" -- $(foreach app,$(APPS),"$(ARTIFACT_DIR)/$(app)")

# Internal flash via STM32CubeProgrammer
PROGRAMMER ?= stm32_programmer_cli
SWD_ARGS ?= port=SWD mode=UR reset=HWrst freq=100
SWD = "$(PROGRAMMER)" -c $(SWD_ARGS)

.PHONY: flash-kernel
# CubeProgrammer recognizes the file format from its extension.
flash-kernel: build
	@set -eu; \
	tmp=$$(mktemp -d); \
	trap 'rm -rf -- "$$tmp"' EXIT; \
	cp -- "$(KERNEL_ELF)" "$$tmp/kernel.elf"; \
	$(SWD) -w "$$tmp/kernel.elf" -v

# External QSPI flash via STM32CubeProgrammer
DEFAULT_LOADER := tools/flash/STM32H743VI_W25Q64.stldr
LOADER_URL := https://raw.githubusercontent.com/manoloaterol/MCUDEV_DevEBox_H743-W25Q64-EL/master/STM32H743VI_W25Q64.stldr
LOADER_SHA256 := 06710874eab2ad7605825194e061ab597e69004d1d032c51e8cdd462693d7c77

QSPI_LOADER ?= $(DEFAULT_LOADER)
QSPI_ADDR ?= 0x90000000
QSPI_SIZE ?= 4096
QSPI_OUTPUT ?= $(OUTPUT_DIR)/qspi-read.bin

.PHONY: qspi-loader qspi-read qspi-write
qspi-loader:
	@set -eu; \
	loader="$(DEFAULT_LOADER)"; \
	if [ ! -f "$$loader" ]; then \
		mkdir -p "$$(dirname "$$loader")"; \
		tmp=$$(mktemp "$$loader.XXXXXX"); \
		trap 'rm -f -- "$$tmp"' EXIT; \
		curl -fsSL -o "$$tmp" "$(LOADER_URL)"; \
		printf '%s  %s\n' "$(LOADER_SHA256)" "$$tmp" | sha256sum -c -; \
		mv -- "$$tmp" "$$loader"; \
	else \
		printf '%s  %s\n' "$(LOADER_SHA256)" "$$loader" | sha256sum -c -; \
	fi

qspi-read:
	@set -eu; \
	test -f "$(QSPI_LOADER)" || { echo "QSPI loader missing. Run make qspi-loader or set QSPI_LOADER." >&2; exit 1; }; \
	mkdir -p "$$(dirname "$(QSPI_OUTPUT)")"; \
	tmp=$$(mktemp -d "$$(dirname "$(QSPI_OUTPUT)")/.qspi-read.XXXXXX"); \
	trap 'rm -rf -- "$$tmp"' EXIT; \
	$(SWD) -el "$$(realpath -- "$(QSPI_LOADER)")" --upload "$(QSPI_ADDR)" "$(QSPI_SIZE)" "$$tmp/read.bin"; \
	test "$$(wc -c < "$$tmp/read.bin")" -eq "$(QSPI_SIZE)"; \
	mv -- "$$tmp/read.bin" "$(QSPI_OUTPUT)"

qspi-write:
	@set -eu; \
	test -f "$(QSPI_LOADER)" || { echo "QSPI loader missing. Run make qspi-loader or set QSPI_LOADER." >&2; exit 1; }; \
	test -f "$(QSPI_IMAGE)" && test -s "$(QSPI_IMAGE)" || { echo "Missing or empty QSPI image: $(QSPI_IMAGE)" >&2; exit 1; }; \
	addr=$$(printf '%u' "$(QSPI_ADDR)"); \
	size=$$(wc -c < "$(QSPI_IMAGE)"); \
	if [ "$$addr" -lt $$((0x90000000)) ] || [ "$$((addr % 4096))" -ne 0 ] || \
	   [ "$$addr" -ge $$((0x90800000)) ] || [ "$$size" -gt "$$((0x90800000 - addr))" ]; then \
		echo "The QSPI image must start on a 4 KiB sector and fit in the 8 MiB NOR." >&2; exit 1; \
	fi; \
	tmp=$$(mktemp -d); \
	trap 'rm -rf -- "$$tmp"' EXIT; \
	cp -- "$(QSPI_IMAGE)" "$$tmp/image.bin"; \
	loader=$$(realpath -- "$(QSPI_LOADER)"); \
	$(SWD) -hardRst; \
	$(SWD) -el "$$loader" -w "$$tmp/image.bin" "$(QSPI_ADDR)" -v
