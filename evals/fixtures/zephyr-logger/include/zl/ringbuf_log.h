/*
 * Sample ring buffer: a fixed-capacity FIFO of sensor samples.
 *
 * The caller owns the storage array. When the buffer is full, the policy
 * chosen at init decides what a put does: ZL_RB_REJECT_NEW refuses the new
 * sample, ZL_RB_DROP_OLDEST discards the oldest sample to make room.
 *
 * All functions are safe to call from threads and ISRs concurrently.
 */

#ifndef ZL_RINGBUF_LOG_H_
#define ZL_RINGBUF_LOG_H_

#include <stddef.h>
#include <stdint.h>
#include <zephyr/spinlock.h>

#ifdef __cplusplus
extern "C" {
#endif

struct zl_sample {
	uint32_t timestamp_ms;
	uint16_t channel;
	int32_t value;
};

enum zl_rb_policy {
	ZL_RB_REJECT_NEW,
	ZL_RB_DROP_OLDEST,
};

struct zl_ringbuf {
	struct zl_sample *records;
	size_t capacity;
	size_t head;
	size_t tail;
	size_t count;
	uint32_t dropped;
	enum zl_rb_policy policy;
	struct k_spinlock lock;
};

typedef void (*zl_ringbuf_visit_t)(const struct zl_sample *sample, void *user_data);

/**
 * Initialise an empty buffer over caller-owned storage.
 *
 * @retval 0 on success
 * @retval -EINVAL if rb or storage is NULL, capacity is 0, or policy is unknown
 */
int zl_ringbuf_init(struct zl_ringbuf *rb, struct zl_sample *storage, size_t capacity,
		    enum zl_rb_policy policy);

/**
 * Append a sample.
 *
 * @retval 0 on success, including when the oldest sample was dropped
 * @retval -ENOSPC if the buffer is full and the policy is ZL_RB_REJECT_NEW
 * @retval -EINVAL if rb or sample is NULL
 */
int zl_ringbuf_put(struct zl_ringbuf *rb, const struct zl_sample *sample);

/**
 * Remove the oldest sample and copy it to out.
 *
 * @retval 0 on success
 * @retval -EAGAIN if the buffer is empty
 * @retval -EINVAL if rb or out is NULL
 */
int zl_ringbuf_get(struct zl_ringbuf *rb, struct zl_sample *out);

/**
 * Copy the oldest sample to out without removing it.
 *
 * @retval 0 on success
 * @retval -EAGAIN if the buffer is empty
 * @retval -EINVAL if rb or out is NULL
 */
int zl_ringbuf_peek(struct zl_ringbuf *rb, struct zl_sample *out);

/**
 * Copy the sample at position index (0 is the oldest) without removing it.
 *
 * @retval 0 on success
 * @retval -ERANGE if index >= count
 * @retval -EINVAL if rb or out is NULL
 */
int zl_ringbuf_peek_at(struct zl_ringbuf *rb, size_t index, struct zl_sample *out);

/**
 * Remove every sample, oldest first, passing each to visit.
 *
 * @return the number of samples removed
 */
size_t zl_ringbuf_drain(struct zl_ringbuf *rb, zl_ringbuf_visit_t visit, void *user_data);

/** Number of samples currently held. */
size_t zl_ringbuf_count(struct zl_ringbuf *rb);

/** Maximum number of samples the buffer holds. */
size_t zl_ringbuf_capacity(const struct zl_ringbuf *rb);

/** Number of samples discarded by ZL_RB_DROP_OLDEST since init or reset. */
uint32_t zl_ringbuf_dropped(struct zl_ringbuf *rb);

/** Empty the buffer and clear the dropped counter. */
void zl_ringbuf_reset(struct zl_ringbuf *rb);

#ifdef __cplusplus
}
#endif

#endif /* ZL_RINGBUF_LOG_H_ */
