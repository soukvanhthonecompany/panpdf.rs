/* RFC 1951 deflate decoding, written here so the installer needs no library.
 *
 * The installer's payload is compressed on the build machine with Python's
 * `zlib` in raw mode and expanded again by this file. It reads a whole buffer
 * into a whole buffer: there is no streaming, because the installer knows every
 * file's uncompressed length before it starts.
 *
 * `inflate_raw` returns 0 on success and a negative number on any malformed
 * input. It never writes past `out_len` and never reads past `in_len`, so a
 * corrupt download fails closed instead of scribbling. */

#include "inflate.h"

#define MAX_BITS 15

struct huffman {
    short counts[MAX_BITS + 1]; /* how many codes of each length */
    short symbols[288];         /* the symbols, ordered by code */
};

struct state {
    const unsigned char *in;
    unsigned long in_len, in_at;
    unsigned char *out;
    unsigned long out_len, out_at;
    unsigned long bit_buffer;
    int bit_count;
};

static int bits(struct state *s, int need)
{
    unsigned long value = s->bit_buffer;
    while (s->bit_count < need) {
        if (s->in_at >= s->in_len) return -1;
        value |= (unsigned long)s->in[s->in_at++] << s->bit_count;
        s->bit_count += 8;
    }
    s->bit_buffer = value >> need;
    s->bit_count -= need;
    return (int)(value & ((1UL << need) - 1));
}

static int decode(struct state *s, const struct huffman *h)
{
    int code = 0, first = 0, index = 0, length;
    for (length = 1; length <= MAX_BITS; length++) {
        int bit = bits(s, 1);
        if (bit < 0) return -1;
        code |= bit;
        int count = h->counts[length];
        if (code - count < first) return h->symbols[index + (code - first)];
        index += count;
        first = (first + count) << 1;
        code <<= 1;
    }
    return -1;
}

static int build(struct huffman *h, const short *lengths, int count)
{
    int i, left, offsets[MAX_BITS + 1];
    for (i = 0; i <= MAX_BITS; i++) h->counts[i] = 0;
    for (i = 0; i < count; i++) h->counts[lengths[i]]++;
    if (h->counts[0] == count) return -1; /* no code at all */
    left = 1;
    for (i = 1; i <= MAX_BITS; i++) {
        left <<= 1;
        left -= h->counts[i];
        if (left < 0) return -1; /* over-subscribed */
    }
    offsets[1] = 0;
    for (i = 1; i < MAX_BITS; i++) offsets[i + 1] = offsets[i] + h->counts[i];
    for (i = 0; i < count; i++)
        if (lengths[i]) h->symbols[offsets[lengths[i]]++] = (short)i;
    return 0;
}

static const short LENGTH_BASE[29] = {3,4,5,6,7,8,9,10,11,13,15,17,19,23,27,31,
                                      35,43,51,59,67,83,99,115,131,163,195,227,258};
static const short LENGTH_EXTRA[29] = {0,0,0,0,0,0,0,0,1,1,1,1,2,2,2,2,
                                       3,3,3,3,4,4,4,4,5,5,5,5,0};
static const short DIST_BASE[30] = {1,2,3,4,5,7,9,13,17,25,33,49,65,97,129,193,
                                    257,385,513,769,1025,1537,2049,3073,4097,6145,
                                    8193,12289,16385,24577};
static const short DIST_EXTRA[30] = {0,0,0,0,1,1,2,2,3,3,4,4,5,5,6,6,
                                     7,7,8,8,9,9,10,10,11,11,12,12,13,13};

static int block(struct state *s, const struct huffman *lit, const struct huffman *dist)
{
    for (;;) {
        int symbol = decode(s, lit);
        if (symbol < 0) return -2;
        if (symbol < 256) {
            if (s->out_at >= s->out_len) return -3;
            s->out[s->out_at++] = (unsigned char)symbol;
        } else if (symbol == 256) {
            return 0;
        } else {
            symbol -= 257;
            if (symbol >= 29) return -4;
            int extra = bits(s, LENGTH_EXTRA[symbol]);
            if (extra < 0) return -4;
            unsigned long length = (unsigned long)LENGTH_BASE[symbol] + (unsigned long)extra;

            symbol = decode(s, dist);
            if (symbol < 0 || symbol >= 30) return -5;
            extra = bits(s, DIST_EXTRA[symbol]);
            if (extra < 0) return -5;
            unsigned long back = (unsigned long)DIST_BASE[symbol] + (unsigned long)extra;

            if (back > s->out_at) return -6;
            if (length > s->out_len - s->out_at) return -3;
            while (length--) {
                s->out[s->out_at] = s->out[s->out_at - back];
                s->out_at++;
            }
        }
    }
}

static int fixed_block(struct state *s)
{
    static struct huffman lit, dist;
    static int ready = 0;
    if (!ready) {
        short lengths[288];
        int i;
        for (i = 0; i < 144; i++) lengths[i] = 8;
        for (; i < 256; i++) lengths[i] = 9;
        for (; i < 280; i++) lengths[i] = 7;
        for (; i < 288; i++) lengths[i] = 8;
        if (build(&lit, lengths, 288)) return -7;
        for (i = 0; i < 30; i++) lengths[i] = 5;
        if (build(&dist, lengths, 30)) return -7;
        ready = 1;
    }
    return block(s, &lit, &dist);
}

static int dynamic_block(struct state *s)
{
    static const short ORDER[19] = {16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15};
    short lengths[288 + 30];
    struct huffman lit, dist;
    int i;

    int nlen = bits(s, 5), ndist = bits(s, 5), ncode = bits(s, 4);
    if (nlen < 0 || ndist < 0 || ncode < 0) return -8;
    nlen += 257; ndist += 1; ncode += 4;
    if (nlen > 286 || ndist > 30) return -8;

    for (i = 0; i < 19; i++) lengths[i] = 0;
    for (i = 0; i < ncode; i++) {
        int value = bits(s, 3);
        if (value < 0) return -8;
        lengths[ORDER[i]] = (short)value;
    }
    if (build(&lit, lengths, 19)) return -8;

    i = 0;
    while (i < nlen + ndist) {
        int symbol = decode(s, &lit), length, repeat;
        if (symbol < 0) return -9;
        if (symbol < 16) {
            lengths[i++] = (short)symbol;
            continue;
        }
        if (symbol == 16) {
            if (i == 0) return -9;
            length = lengths[i - 1];
            repeat = bits(s, 2);
            if (repeat < 0) return -9;
            repeat += 3;
        } else if (symbol == 17) {
            length = 0;
            repeat = bits(s, 3);
            if (repeat < 0) return -9;
            repeat += 3;
        } else {
            length = 0;
            repeat = bits(s, 7);
            if (repeat < 0) return -9;
            repeat += 11;
        }
        if (i + repeat > nlen + ndist) return -9;
        while (repeat--) lengths[i++] = (short)length;
    }
    if (lengths[256] == 0) return -9; /* no end-of-block code */

    if (build(&lit, lengths, nlen)) return -9;
    if (build(&dist, lengths + nlen, ndist)) return -9;
    return block(s, &lit, &dist);
}

static int stored_block(struct state *s)
{
    s->bit_buffer = 0;
    s->bit_count = 0;
    if (s->in_at + 4 > s->in_len) return -10;
    unsigned long length = (unsigned long)s->in[s->in_at] | ((unsigned long)s->in[s->in_at + 1] << 8);
    unsigned long check = (unsigned long)s->in[s->in_at + 2] | ((unsigned long)s->in[s->in_at + 3] << 8);
    if ((length ^ 0xFFFF) != check) return -10;
    s->in_at += 4;
    if (s->in_at + length > s->in_len) return -10;
    if (length > s->out_len - s->out_at) return -3;
    while (length--) s->out[s->out_at++] = s->in[s->in_at++];
    return 0;
}

int inflate_raw(const unsigned char *in, unsigned long in_len,
                unsigned char *out, unsigned long out_len)
{
    struct state s;
    s.in = in; s.in_len = in_len; s.in_at = 0;
    s.out = out; s.out_len = out_len; s.out_at = 0;
    s.bit_buffer = 0; s.bit_count = 0;

    for (;;) {
        int last = bits(&s, 1);
        int kind = bits(&s, 2);
        int failed;
        if (last < 0 || kind < 0) return -11;
        if (kind == 0) failed = stored_block(&s);
        else if (kind == 1) failed = fixed_block(&s);
        else if (kind == 2) failed = dynamic_block(&s);
        else return -12;
        if (failed) return failed;
        if (last) break;
    }
    return s.out_at == out_len ? 0 : -13;
}
