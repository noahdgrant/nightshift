#include <errno.h>

#include <zephyr/ztest.h>

#include <zl/ringbuf_log.h>
#include <zl/sensor_reader.h>

#define CAPACITY 8

struct fake_sensor {
	int ready;
	int read_error;
	int32_t raw;
	uint16_t last_channel;
	uint32_t now_ms;
	int init_calls;
};

static int fake_init(void *ctx)
{
	struct fake_sensor *fake = ctx;

	fake->init_calls++;
	return 0;
}

static int fake_data_ready(void *ctx)
{
	struct fake_sensor *fake = ctx;

	return fake->ready;
}

static int fake_read_raw(void *ctx, uint16_t channel, int32_t *raw)
{
	struct fake_sensor *fake = ctx;

	fake->last_channel = channel;
	if (fake->read_error != 0) {
		return fake->read_error;
	}
	*raw = fake->raw;
	return 0;
}

static uint32_t fake_uptime_ms(void *ctx)
{
	struct fake_sensor *fake = ctx;

	return fake->now_ms;
}

static const struct zl_sensor_hal fake_hal = {
	.init = fake_init,
	.data_ready = fake_data_ready,
	.read_raw = fake_read_raw,
	.uptime_ms = fake_uptime_ms,
};

static const struct zl_sensor_reader_config cfg = {
	.channel = 3U,
	.scale_num = 1,
	.scale_den = 10,
};

static struct fake_sensor fake;
static struct zl_sample storage[CAPACITY];
static struct zl_ringbuf log;
static struct zl_sensor_reader reader;

static void before(void *fixture)
{
	ARG_UNUSED(fixture);
	fake = (struct fake_sensor){.ready = 1, .raw = 215, .now_ms = 1000U};
	zassert_ok(zl_ringbuf_init(&log, storage, CAPACITY, ZL_RB_REJECT_NEW));
	zassert_ok(zl_sensor_reader_init(&reader, &fake_hal, &fake, &log, &cfg));
}

ZTEST_SUITE(sensor_reader, NULL, NULL, before, NULL, NULL);

ZTEST(sensor_reader, test_init_runs_the_hal_init)
{
	zassert_equal(fake.init_calls, 1);
}

ZTEST(sensor_reader, test_init_rejects_incomplete_hal)
{
	struct zl_sensor_hal no_clock = fake_hal;
	struct zl_sensor_reader_config zero_den = cfg;

	no_clock.uptime_ms = NULL;
	zero_den.scale_den = 0;

	zassert_equal(zl_sensor_reader_init(&reader, &no_clock, &fake, &log, &cfg), -EINVAL);
	zassert_equal(zl_sensor_reader_init(&reader, &fake_hal, &fake, &log, &zero_den), -EINVAL);
	zassert_equal(zl_sensor_reader_init(&reader, NULL, &fake, &log, &cfg), -EINVAL);
}

ZTEST(sensor_reader, test_sample_logs_a_scaled_timestamped_reading)
{
	struct zl_sample out;

	zassert_ok(zl_sensor_reader_sample(&reader));

	zassert_equal(fake.last_channel, 3U);
	zassert_ok(zl_ringbuf_get(&log, &out));
	zassert_equal(out.timestamp_ms, 1000U);
	zassert_equal(out.channel, 3U);
	zassert_equal(out.value, 21);
}

ZTEST(sensor_reader, test_sample_returns_eagain_when_not_ready)
{
	fake.ready = 0;

	zassert_equal(zl_sensor_reader_sample(&reader), -EAGAIN);
	zassert_equal(zl_ringbuf_count(&log), 0);
	zassert_equal(zl_sensor_reader_errors(&reader), 0);
}

ZTEST(sensor_reader, test_read_error_is_returned_and_counted)
{
	fake.read_error = -EIO;

	zassert_equal(zl_sensor_reader_sample(&reader), -EIO);
	zassert_equal(zl_ringbuf_count(&log), 0);
	zassert_equal(zl_sensor_reader_errors(&reader), 1);
}

ZTEST(sensor_reader, test_full_log_is_reported_as_an_error)
{
	for (int i = 0; i < CAPACITY; i++) {
		zassert_ok(zl_sensor_reader_sample(&reader));
	}

	zassert_equal(zl_sensor_reader_sample(&reader), -ENOSPC);
	zassert_equal(zl_sensor_reader_errors(&reader), 1);
}
