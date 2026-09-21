/* Prove the installer's decompressor before it is shipped inside one.
 *
 *     inflate_test cases.bin
 *
 * `cases.bin` is written by `pack.py --self-test`, which compresses known
 * answers with Python's zlib at several levels and strategies, so all three
 * block kinds -- stored, fixed Huffman and dynamic Huffman -- are exercised.
 * Each case carries the answer it must produce. The file ends with deliberately
 * corrupt cases, which must fail rather than produce anything.
 *
 * Exit 0 if every case behaved; 1 and a line naming the case otherwise. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "inflate.h"

static unsigned long u32(const unsigned char *p)
{
    return (unsigned long)p[0] | ((unsigned long)p[1] << 8)
         | ((unsigned long)p[2] << 16) | ((unsigned long)p[3] << 24);
}

int main(int argc, char **argv)
{
    if (argc != 2) { fprintf(stderr, "usage: inflate_test cases.bin\n"); return 2; }
    FILE *f = fopen(argv[1], "rb");
    if (!f) { perror(argv[1]); return 2; }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    unsigned char *blob = malloc((size_t)size);
    if (!blob || fread(blob, 1, (size_t)size, f) != (size_t)size) { fprintf(stderr, "short read\n"); return 2; }
    fclose(f);

    unsigned long at = 0, cases = u32(blob), passed = 0;
    at = 4;
    for (unsigned long i = 0; i < cases; i++) {
        unsigned long name_len = u32(blob + at); at += 4;
        char name[128];
        unsigned long keep = name_len < sizeof name - 1 ? name_len : sizeof name - 1;
        memcpy(name, blob + at, keep); name[keep] = 0; at += name_len;
        int must_fail = blob[at]; at += 1;
        unsigned long comp_len = u32(blob + at); at += 4;
        unsigned long raw_len = u32(blob + at); at += 4;
        const unsigned char *comp = blob + at; at += comp_len;
        const unsigned char *want = blob + at; at += raw_len;

        unsigned char *got = malloc(raw_len ? raw_len : 1);
        int failed = inflate_raw(comp, comp_len, got, raw_len);
        if (must_fail) {
            if (!failed) { printf("FAIL %s: corrupt input was accepted\n", name); return 1; }
        } else if (failed) {
            printf("FAIL %s: inflate_raw returned %d\n", name, failed);
            return 1;
        } else if (raw_len && memcmp(got, want, raw_len) != 0) {
            printf("FAIL %s: %lu bytes out, but not the right ones\n", name, raw_len);
            return 1;
        }
        free(got);
        passed++;
    }
    printf("  inflate: %lu known answers, all correct\n", passed);
    return 0;
}
