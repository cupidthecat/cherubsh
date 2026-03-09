CC := cc
CFLAGS := -std=c11 -Wall -Wextra -Werror -pedantic -g -Iinclude -D_POSIX_C_SOURCE=200809L
LDFLAGS :=

SRC := $(wildcard src/*.c)
OBJ := $(patsubst src/%.c,build/%.o,$(SRC))
BIN := build/cupid

INTEGRATION_TESTS := $(filter-out tests/integration/test_full_phase1.sh,$(wildcard tests/integration/test_*.sh))
UNIT_TESTS := $(wildcard tests/unit/test_*.c)
UNIT_BINS := $(patsubst tests/unit/%.c,build/%,$(UNIT_TESTS))

.PHONY: all clean test test-unit test-integration test-full test-parity-bash test-parity-posix test-parity-advanced test-parity-posix-full test-parity-bash-full test-compat-bash-5.2.21 test-bash521-core test-bash521-core-file test-bash521-core-c test-bash521-wave2-file

all: $(BIN)

$(BIN): $(OBJ)
	@mkdir -p build
	$(CC) $(CFLAGS) $(OBJ) -o $@ $(LDFLAGS)

build/%.o: src/%.c
	@mkdir -p build
	$(CC) $(CFLAGS) -c $< -o $@

build/test_%: tests/unit/test_%.c $(filter-out build/main.o,$(OBJ))
	@mkdir -p build
	$(CC) $(CFLAGS) $< $(filter-out build/main.o,$(OBJ)) -o $@ $(LDFLAGS)

test: test-unit test-integration

test-unit: all $(UNIT_BINS)
	@set -e; for t in $(UNIT_BINS); do echo "RUN $$t"; "$$t"; done

test-integration: all
	@set -e; for t in $(INTEGRATION_TESTS); do echo "RUN $$t"; bash "$$t"; done

test-full:
	bash tests/integration/test_full_phase1.sh

test-parity-bash: all
	bash tests/parity/run_parity.sh bash tests/parity/bash_core_cases.sh

test-parity-posix: all
	bash tests/parity/run_parity.sh posix tests/parity/posix_core_cases.sh

test-parity-advanced: all
	bash tests/parity/run_parity.sh advanced tests/parity/bash_advanced_cases.sh

test-parity-posix-full: all
	bash tests/parity/run_parity.sh posix tests/parity/posix_full_cases.sh

test-parity-bash-full: all
	bash tests/parity/run_parity.sh bash tests/parity/bash_full_cases.sh

test-compat-bash-5.2.21: all
	bash tests/compat/run_bash_5_2_21_coverage.sh

test-bash521-core: all
	bash tests/compat/run_bash_5_2_21_native.sh --exec-mode both

test-bash521-core-file: all
	bash tests/compat/run_bash_5_2_21_native.sh --exec-mode file

test-bash521-core-c: all
	bash tests/compat/run_bash_5_2_21_native.sh --exec-mode c

test-bash521-wave2-file: all
	bash tests/compat/run_bash_5_2_21_native.sh --exec-mode file \
		--runner run-tilde \
		--runner run-history \
		--runner run-histexpand \
		--runner run-varenv \
		--runner run-vredir \
		--runner run-dollars \
		--runner run-braces \
		--runner run-appendop \
		--runner run-arith-for

clean:
	rm -rf build
