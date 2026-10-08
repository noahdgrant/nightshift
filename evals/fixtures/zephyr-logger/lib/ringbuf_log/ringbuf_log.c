#include <errno.h>

#include <zl/ringbuf_log.h>

static size_t advance(const struct zl_ringbuf *rb, size_t index)
{
	return (index + 1U) % rb->capacity;
}

int zl_ringbuf_init(struct zl_ringbuf *rb, struct zl_sample *storage, size_t capacity,
		    enum zl_rb_policy policy)
{
	if (rb == NULL || storage == NULL || capacity == 0U) {
		return -EINVAL;
	}
	if (policy != ZL_RB_REJECT_NEW && policy != ZL_RB_DROP_OLDEST) {
		return -EINVAL;
	}

	rb->records = storage;
	rb->capacity = capacity;
	rb->head = 0U;
	rb->tail = 0U;
	rb->count = 0U;
	rb->dropped = 0U;
	rb->policy = policy;
	rb->lock = (struct k_spinlock){};

	return 0;
}

int zl_ringbuf_put(struct zl_ringbuf *rb, const struct zl_sample *sample)
{
	if (rb == NULL || sample == NULL) {
		return -EINVAL;
	}

	k_spinlock_key_t key = k_spin_lock(&rb->lock);

	if (rb->count == rb->capacity) {
		if (rb->policy == ZL_RB_REJECT_NEW) {
			k_spin_unlock(&rb->lock, key);
			return -ENOSPC;
		}
		rb->dropped++;
	}

	rb->records[rb->head] = *sample;
	rb->head = advance(rb, rb->head);
	rb->count++;

	k_spin_unlock(&rb->lock, key);

	return 0;
}

int zl_ringbuf_get(struct zl_ringbuf *rb, struct zl_sample *out)
{
	if (rb == NULL || out == NULL) {
		return -EINVAL;
	}

	k_spinlock_key_t key = k_spin_lock(&rb->lock);

	if (rb->count == 0U) {
		k_spin_unlock(&rb->lock, key);
		return -EAGAIN;
	}

	*out = rb->records[rb->tail];
	rb->tail = advance(rb, rb->tail);
	rb->count--;

	k_spin_unlock(&rb->lock, key);

	return 0;
}

int zl_ringbuf_peek(struct zl_ringbuf *rb, struct zl_sample *out)
{
	if (rb == NULL || out == NULL) {
		return -EINVAL;
	}

	int ret = zl_ringbuf_peek_at(rb, 0U, out);

	return ret == -ERANGE ? -EAGAIN : ret;
}

int zl_ringbuf_peek_at(struct zl_ringbuf *rb, size_t index, struct zl_sample *out)
{
	if (rb == NULL || out == NULL) {
		return -EINVAL;
	}

	k_spinlock_key_t key = k_spin_lock(&rb->lock);

	if (index >= rb->count) {
		k_spin_unlock(&rb->lock, key);
		return -ERANGE;
	}

	*out = rb->records[(rb->tail + index) % rb->capacity];

	k_spin_unlock(&rb->lock, key);

	return 0;
}

size_t zl_ringbuf_drain(struct zl_ringbuf *rb, zl_ringbuf_visit_t visit, void *user_data)
{
	struct zl_sample sample;
	size_t drained = 0U;

	if (rb == NULL) {
		return 0U;
	}

	while (zl_ringbuf_get(rb, &sample) == 0) {
		if (visit != NULL) {
			visit(&sample, user_data);
		}
		drained++;
	}

	return drained;
}

size_t zl_ringbuf_count(struct zl_ringbuf *rb)
{
	k_spinlock_key_t key = k_spin_lock(&rb->lock);
	size_t count = rb->count;

	k_spin_unlock(&rb->lock, key);

	return count;
}

size_t zl_ringbuf_capacity(const struct zl_ringbuf *rb)
{
	return rb->capacity;
}

uint32_t zl_ringbuf_dropped(struct zl_ringbuf *rb)
{
	k_spinlock_key_t key = k_spin_lock(&rb->lock);
	uint32_t dropped = rb->dropped;

	k_spin_unlock(&rb->lock, key);

	return dropped;
}

void zl_ringbuf_reset(struct zl_ringbuf *rb)
{
	k_spinlock_key_t key = k_spin_lock(&rb->lock);

	rb->head = 0U;
	rb->tail = 0U;
	rb->count = 0U;
	rb->dropped = 0U;

	k_spin_unlock(&rb->lock, key);
}
