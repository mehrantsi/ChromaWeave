#ifndef CW_CODEC_H
#define CW_CODEC_H
#include <stddef.h>
#include <stdint.h>

// Input buffers must remain valid for the duration of the call. Free returned strings with cw_string_free.
char *cw_decode_bgra(const uint8_t *pixels, size_t length, uint32_t width, uint32_t height, size_t stride);
char *cw_transfer_repair(uint8_t data_shards, uint32_t shard_bytes, const uint8_t *indices,
                         size_t shard_count, const uint8_t *packed, size_t length);
void *cw_recovery_create(void);
void cw_recovery_free(void *session);
char *cw_recovery_decode_bgra(void *session, const uint8_t *pixels, size_t length,
                            uint32_t width, uint32_t height, size_t stride);
void *cw_transfer_scanner_create(void);
void cw_transfer_scanner_reset(void *session);
void cw_transfer_scanner_free(void *session);
char *cw_transfer_scanner_decode_bgra(void *session, const uint8_t *pixels, size_t length,
                                      uint32_t width, uint32_t height, size_t stride);
char *cw_encode_png(const uint8_t *payload, size_t length, uint32_t profile, uint32_t cell_pixels);
/* compression: 0=none, 1=auto */
char *cw_encode_png_with_options(const uint8_t *payload, size_t length,
                                 uint32_t profile, uint32_t cell_pixels,
                                 uint32_t compression);
void cw_string_free(char *value);
#endif
