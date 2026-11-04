/*
 * Copyright (c) 2026, the Exile OS authors
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// NOT the Exile prekernel. A minimal freestanding UEFI application used only to validate the QEMU test runner's
// mechanics (OVMF boots it, ConOut text reaches the -serial capture, timeouts and exit codes are classified).
// It prints through EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL.OutputString, the same channel Kernel/EFIPrekernel/DebugOutput.cpp
// (dbgputstr) uses. Behaviour is selected at build time with -DSTANDIN_MODE=<n>:
//   0: print the "verified" line and halt        1: print the "verification failed" line and halt
//   2: print nothing and halt                    3: print the "verified" line, then return to firmware (no halt)
typedef unsigned long long UINTN;
typedef unsigned short CHAR16;
typedef struct {
    void* Reset;
    UINTN (*OutputString)(void* self, CHAR16* string);
} SimpleTextOutput;
typedef struct {
    char Hdr[24];
    CHAR16* FirmwareVendor;
    unsigned FirmwareRevision;
    void* ConsoleInHandle;
    void* ConIn;
    void* ConsoleOutHandle;
    SimpleTextOutput* ConOut;
} SystemTable;

#ifndef STANDIN_MODE
#    define STANDIN_MODE 0
#endif

#define VERIFIED u"Kernel image verified against the signed boot manifest (STANDIN)\r\n"
#define FAILED u"Kernel image verification failed: STANDIN\r\n"

UINTN efi_main(void* image, SystemTable* st)
{
    (void)image;
#if STANDIN_MODE == 0 || STANDIN_MODE == 3
    st->ConOut->OutputString(st->ConOut, (CHAR16*)VERIFIED);
#elif STANDIN_MODE == 1
    st->ConOut->OutputString(st->ConOut, (CHAR16*)FAILED);
#endif
#if STANDIN_MODE != 3
    for (;;)
        __asm__ volatile("hlt");
#endif
    return 0;
}
