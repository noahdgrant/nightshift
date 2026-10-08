#include <errno.h>

#include <zephyr/ztest.h>

#include <zl/ringbuf_log.h>

#define CAPACITY 4

static struct zl_sample storage[CAPACITY];
static struct zl_ringbuf rb;

static struct zl_sample sample(uint32_t n)
{
	return (struct zl_sample){.timestamp_ms = n * 100U, .channel = 1U, .value = (int32_t)n};
}

static void put_n(uint32_t first, uint32_t n)
{
	for (uint32_t i = first; i < first + n; i++) {
		struct zl_sample s = sample(i);

		zassert_ok(zl_ringbuf_put(&rb, &s));
	}
}

static void assert_get(int32_t expected_value)
{
	struct zl_sample out;

	zassert_ok(zl_ringbuf_get(&rb, &out));
	zassert_equal(out.value, expected_value, "got %d, expected %d", out.value,
		      expected_value);
}

static void before(void *fixture)
{
	ARG_UNUSED(fixture);
	zassert_ok(zl_ringbuf_init(&rb, storage, CAPACITY, ZL_RB_REJECT_NEW));
}

ZTEST_SUITE(ringbuf_log, NULL, NULL, before, NULL, NULL);

ZTEST(ringbuf_log, test_init_rejects_bad_arguments)
{
	zassert_equal(zl_ringbuf_init(NULL, storage, CAPACITY, ZL_RB_REJECT_NEW), -EINVAL);
	zassert_equal(zl_ringbuf_init(&rb, NULL, CAPACITY, ZL_RB_REJECT_NEW), -EINVAL);
	zassert_equal(zl_ringbuf_init(&rb, storage, 0, ZL_RB_REJECT_NEW), -EINVAL);
	zassert_equal(zl_ringbuf_init(&rb, storage, CAPACITY, (enum zl_rb_policy)7), -EINVAL);
}

ZTEST(ringbuf_log, test_new_buffer_is_empty)
{
	struct zl_sample out;

	zassert_equal(zl_ringbuf_count(&rb), 0);
	zassert_equal(zl_ringbuf_capacity(&rb), CAPACITY);
	zassert_equal(zl_ringbuf_get(&rb, &out), -EAGAIN);
	zassert_equal(zl_ringbuf_peek(&rb, &out), -EAGAIN);
}

ZTEST(ringbuf_log, test_get_returns_samples_oldest_first)
{
	put_n(1, 3);

	assert_get(1);
	assert_get(2);
	assert_get(3);
	zassert_equal(zl_ringbuf_count(&rb), 0);
}

ZTEST(ringbuf_log, test_get_copies_the_whole_sample)
{
	struct zl_sample in = {.timestamp_ms = 12345U, .channel = 9U, .value = -40};
	struct zl_sample out;

	zassert_ok(zl_ringbuf_put(&rb, &in));
	zassert_ok(zl_ringbuf_get(&rb, &out));
	zassert_equal(out.timestamp_ms, 12345U);
	zassert_equal(out.channel, 9U);
	zassert_equal(out.value, -40);
}

ZTEST(ringbuf_log, test_peek_leaves_the_sample_in_place)
{
	struct zl_sample out;

	put_n(1, 2);

	zassert_ok(zl_ringbuf_peek(&rb, &out));
	zassert_equal(out.value, 1);
	zassert_equal(zl_ringbuf_count(&rb), 2);
	assert_get(1);
}

ZTEST(ringbuf_log, test_peek_at_indexes_from_the_oldest)
{
	struct zl_sample out;

	put_n(1, 3);

	zassert_ok(zl_ringbuf_peek_at(&rb, 2, &out));
	zassert_equal(out.value, 3);
	zassert_equal(zl_ringbuf_peek_at(&rb, 3, &out), -ERANGE);
}

ZTEST(ringbuf_log, test_reject_new_keeps_contents_when_full)
{
	struct zl_sample extra = sample(99);

	put_n(1, CAPACITY);

	zassert_equal(zl_ringbuf_put(&rb, &extra), -ENOSPC);
	zassert_equal(zl_ringbuf_count(&rb), CAPACITY);
	zassert_equal(zl_ringbuf_dropped(&rb), 0);
	assert_get(1);
}

ZTEST(ringbuf_log, test_order_survives_wraparound)
{
	put_n(1, 3);
	assert_get(1);
	assert_get(2);
	put_n(4, 3);

	zassert_equal(zl_ringbuf_count(&rb), 4);
	assert_get(3);
	assert_get(4);
	assert_get(5);
	assert_get(6);
}

ZTEST(ringbuf_log, test_drop_oldest_accepts_up_to_capacity)
{
	zassert_ok(zl_ringbuf_init(&rb, storage, CAPACITY, ZL_RB_DROP_OLDEST));

	put_n(1, CAPACITY);

	zassert_equal(zl_ringbuf_count(&rb), CAPACITY);
	zassert_equal(zl_ringbuf_dropped(&rb), 0);
	assert_get(1);
}

static void sum_values(const struct zl_sample *s, void *user_data)
{
	*(int32_t *)user_data += s->value;
}

ZTEST(ringbuf_log, test_drain_visits_every_sample_and_empties)
{
	int32_t sum = 0;

	put_n(1, 3);

	zassert_equal(zl_ringbuf_drain(&rb, sum_values, &sum), 3);
	zassert_equal(sum, 6);
	zassert_equal(zl_ringbuf_count(&rb), 0);
}

ZTEST(ringbuf_log, test_reset_empties_the_buffer)
{
	struct zl_sample out;

	put_n(1, 3);
	zl_ringbuf_reset(&rb);

	zassert_equal(zl_ringbuf_count(&rb), 0);
	zassert_equal(zl_ringbuf_get(&rb, &out), -EAGAIN);
}
