/*
 * Polls a simulated temperature sensor every 100 ms and logs the buffered
 * samples once a second.
 */

#include <zephyr/kernel.h>
#include <zephyr/logging/log.h>

#include <zl/ringbuf_log.h>
#include <zl/sensor_reader.h>

LOG_MODULE_REGISTER(zlogger_app, LOG_LEVEL_INF);

#define LOG_CAPACITY 16
#define TEMP_CHANNEL 0

struct sim_sensor {
	int32_t next_raw;
};

static int sim_data_ready(void *ctx)
{
	ARG_UNUSED(ctx);
	return 1;
}

static int sim_read_raw(void *ctx, uint16_t channel, int32_t *raw)
{
	struct sim_sensor *sim = ctx;

	ARG_UNUSED(channel);
	*raw = sim->next_raw;
	sim->next_raw = (sim->next_raw + 7) % 400;
	return 0;
}

static uint32_t sim_uptime_ms(void *ctx)
{
	ARG_UNUSED(ctx);
	return k_uptime_get_32();
}

static const struct zl_sensor_hal sim_hal = {
	.data_ready = sim_data_ready,
	.read_raw = sim_read_raw,
	.uptime_ms = sim_uptime_ms,
};

static void log_sample(const struct zl_sample *sample, void *user_data)
{
	ARG_UNUSED(user_data);
	LOG_INF("t=%u ch=%u value=%d", sample->timestamp_ms, sample->channel, sample->value);
}

int main(void)
{
	static struct zl_sample storage[LOG_CAPACITY];
	static struct zl_ringbuf log;
	static struct zl_sensor_reader reader;
	static struct sim_sensor sim = {.next_raw = 200};
	const struct zl_sensor_reader_config cfg = {
		.channel = TEMP_CHANNEL,
		.scale_num = 1,
		.scale_den = 10,
	};
	int ret;

	ret = zl_ringbuf_init(&log, storage, LOG_CAPACITY, ZL_RB_DROP_OLDEST);
	if (ret < 0) {
		LOG_ERR("ringbuf init failed: %d", ret);
		return ret;
	}

	ret = zl_sensor_reader_init(&reader, &sim_hal, &sim, &log, &cfg);
	if (ret < 0) {
		LOG_ERR("reader init failed: %d", ret);
		return ret;
	}

	for (uint32_t tick = 1U;; tick++) {
		ret = zl_sensor_reader_sample(&reader);
		if (ret < 0 && ret != -EAGAIN) {
			LOG_WRN("sample failed: %d", ret);
		}
		if (tick % 10U == 0U) {
			zl_ringbuf_drain(&log, log_sample, NULL);
		}
		k_msleep(100);
	}

	return 0;
}
