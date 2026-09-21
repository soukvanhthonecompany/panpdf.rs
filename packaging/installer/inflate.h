#ifndef PANPDF_INFLATE_H
#define PANPDF_INFLATE_H

/* Expand `in_len` bytes of raw deflate into exactly `out_len` bytes.
 * 0 on success; a negative number if the input is malformed or does not
 * produce exactly `out_len` bytes. */
int inflate_raw(const unsigned char *in, unsigned long in_len,
                unsigned char *out, unsigned long out_len);

#endif
