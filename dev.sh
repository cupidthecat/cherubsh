#!/bin/bash
# Development build script for cupidshell with strict safety and debugging flags

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Cupidshell Development Build ===${NC}"
echo "Compiling with strict safety flags, sanitizers, and debug symbols..."

# Compiler settings
CC="${CC:-gcc}"
AR="${AR:-ar}"

# Strict development flags
DEV_CFLAGS="-std=c99 -g3 -O0 \
-Wall -Wextra -Werror \
-Wpedantic \
-Wformat=2 \
-Wformat-security \
-Wnull-dereference \
-Wstack-protector \
-Wtrampolines \
-Warray-bounds \
-Wcast-align \
-Wcast-qual \
-Wconversion \
-Wsign-conversion \
-Wstrict-overflow=4 \
-Wundef \
-Wunused \
-Wuninitialized \
-Wshadow \
-Wpointer-arith \
-Wwrite-strings \
-Wswitch-default \
-Wswitch-enum \
-Wmissing-declarations \
-Wmissing-prototypes \
-Wstrict-prototypes \
-Wredundant-decls \
-Wdouble-promotion \
-Wfloat-equal \
-Wvla \
-fstack-protector-strong \
-fno-omit-frame-pointer \
-D_FORTIFY_SOURCE=2"

# Sanitizer flags
SANITIZER_FLAGS="-fsanitize=address -fsanitize=undefined -fsanitize=leak -fsanitize=bounds -fsanitize=null -fsanitize=return"

# Combined flags
CFLAGS="${DEV_CFLAGS} ${SANITIZER_FLAGS}"
LDLIBS="-lm"
INCLUDES="-Isrc -I."

# Directories
OBJDIR="obj"
BINDIR="bin"
SRCDIR="src"

# Clean previous build
echo -e "${YELLOW}Cleaning previous build...${NC}"
rm -rf ${OBJDIR}/*.o ${BINDIR}/libcupidshell.a ${BINDIR}/cupidshell 2>/dev/null || true

# Create directories
mkdir -p ${OBJDIR}
mkdir -p ${BINDIR}

# Source files for library
LIB_SOURCES=(
    "${SRCDIR}/cupidshell.c"
    # update
    # add/remove files here as your project structure changes
)

# Compile library object files
echo -e "${YELLOW}Compiling library object files...${NC}"
for src in "${LIB_SOURCES[@]}"; do
    obj="${OBJDIR}/$(basename ${src%.c}.o)"
    echo "  Compiling $(basename $src)..."
    if ${CC} ${CFLAGS} ${INCLUDES} -c $src -o $obj; then
        echo -e "    ${GREEN}✓${NC} $(basename $src)"
    else
        echo -e "    ${RED}✗ FAILED${NC} $(basename $src)"
        exit 1
    fi
done

# Create static library
echo -e "${YELLOW}Creating static library...${NC}"
LIBOBJ=$(ls ${OBJDIR}/*.o)
if ${AR} rcs ${BINDIR}/libcupidshell.a ${LIBOBJ}; then
    echo -e "  ${GREEN}✓${NC} libcupidshell.a created"
else
    echo -e "  ${RED}✗ FAILED${NC} to create library"
    exit 1
fi

# Compile CLI (main shell) executable
echo -e "${YELLOW}Compiling Cupidshell executable...${NC}"
CLI_SOURCES="${SRCDIR}/cupidshell.c ${SRCDIR}/cupidshell_internal.c ${SRCDIR}/cupidshell_builtin.c ${SRCDIR}/cupidshell_parse.c ${SRCDIR}/cupidshell_exec.c ${SRCDIR}/cupidshell_env.c ${SRCDIR}/cupidshell_util.c ${SRCDIR}/cupidshell_str.c ${SRCDIR}/cupidshell_main.c"

if ${CC} ${CFLAGS} ${INCLUDES} ${CLI_SOURCES} -o ${BINDIR}/cupidshell ${LDLIBS}; then
    echo -e "  ${GREEN}✓${NC} cupidshell executable built"
else
    echo -e "  ${RED}✗ FAILED${NC} to build cupidshell"
    exit 1
fi

echo ""
echo -e "${GREEN}=== Build Successful ===${NC}"
echo "Binary: ${BINDIR}/cupidshell"
echo "Library: ${BINDIR}/libcupidshell.a"
echo ""
echo -e "${YELLOW}Build Configuration:${NC}"
echo "  - Debug symbols: Enabled (g3)"
echo "  - Optimization: Disabled (O0)"
echo "  - All warnings: Enabled"
echo "  - Warnings as errors: Enabled"
echo "  - AddressSanitizer: Enabled"
echo "  - UndefinedBehaviorSanitizer: Enabled"
echo "  - LeakSanitizer: Enabled"
echo "  - Stack protector: Enabled"
echo ""
echo -e "${YELLOW}Usage:${NC}"
echo "  Run shell: ${BINDIR}/cupidshell"
echo ""
echo -e "${YELLOW}Note:${NC} Sanitizers will report any memory errors, leaks, or undefined behavior at runtime."
