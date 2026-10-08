#include <errno.h>
#include <stddef.h>

#include <zl/sensor_reader.h>

int zl_sensor_reader_init(struct zl_sensor_reader *reader, const struct zl_sensor_hal *hal,
			  void *ctx, struct zl_ringbuf *log,
			  const struct zl_sensor_reader_config *cfg)
{
	if (reader == NULL || hal == NULL || log == NULL || cfg == NULL) {
		return -EINVAL;
	}
	if (hal->data_ready == NULL || hal->read_raw == NULL || hal->uptime_ms == NULL) {
		return -EINVAL;
	}
	if (cfg->scale_den == 0) {
		return -EINVAL;
	}

	reader->hal = hal;
	reader->ctx = ctx;
	reader->log = log;
	reader->cfg = *cfg;
	reader->errors = 0U;

	if (hal->init != NULL) {
		return hal->init(ctx);
	}

	return 0;
}

int zl_sensor_reader_sample(struct zl_sensor_reader *reader)
{
	int32_t raw;
	int ret;

	ret = reader->hal->data_ready(reader->ctx);
	if (ret == 0) {
		return -EAGAIN;
	}
	if (ret < 0) {
		reader->errors++;
		return ret;
	}

	ret = reader->hal->read_raw(reader->ctx, reader->cfg.channel, &raw);
	if (ret < 0) {
		reader->errors++;
		return ret;
	}

	struct zl_sample sample = {
		.timestamp_ms = reader->hal->uptime_ms(reader->ctx),
		.channel = reader->cfg.channel,
		.value = (int32_t)(((int64_t)raw * reader->cfg.scale_num) / reader->cfg.scale_den),
	};

	ret = zl_ringbuf_put(reader->log, &sample);
	if (ret < 0) {
		reader->errors++;
	}

	return ret;
}

uint32_t zl_sensor_reader_errors(const struct zl_sensor_reader *reader)
{
	return reader->errors;
}
