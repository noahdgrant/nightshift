/*
 * Sensor reader: polls one sensor channel through a HAL and appends each
 * reading to a sample ring buffer as a timestamped, scaled sample.
 *
 * The HAL is a table of function pointers so tests can drive the reader
 * with a fake sensor and a fake clock.
 */

#ifndef ZL_SENSOR_READER_H_
#define ZL_SENSOR_READER_H_

#include <stdint.h>

#include <zl/ringbuf_log.h>

#ifdef __cplusplus
extern "C" {
#endif

struct zl_sensor_hal {
	/** Prepare the sensor. Optional: NULL means nothing to do. */
	int (*init)(void *ctx);
	/** Return 1 if a reading is ready, 0 if not, or a negative errno. */
	int (*data_ready)(void *ctx);
	/** Read the raw value of a channel. Return 0 or a negative errno. */
	int (*read_raw)(void *ctx, uint16_t channel, int32_t *raw);
	/** Milliseconds since boot. */
	uint32_t (*uptime_ms)(void *ctx);
};

struct zl_sensor_reader_config {
	uint16_t channel;
	/** Sample value = raw * scale_num / scale_den. */
	int32_t scale_num;
	int32_t scale_den;
};

struct zl_sensor_reader {
	const struct zl_sensor_hal *hal;
	void *ctx;
	struct zl_ringbuf *log;
	struct zl_sensor_reader_config cfg;
	uint32_t errors;
};

/**
 * Bind a reader to a HAL and a ring buffer, and run the HAL's init.
 *
 * @retval 0 on success
 * @retval -EINVAL if a pointer is NULL, the HAL lacks data_ready, read_raw or
 *         uptime_ms, or scale_den is 0
 * @retval the HAL init's error if it fails
 */
int zl_sensor_reader_init(struct zl_sensor_reader *reader, const struct zl_sensor_hal *hal,
			  void *ctx, struct zl_ringbuf *log,
			  const struct zl_sensor_reader_config *cfg);

/**
 * Take one reading if the sensor has one ready, and append it to the log.
 *
 * Does not wait: if no reading is ready it returns -EAGAIN at once.
 *
 * @retval 0 on success
 * @retval -EAGAIN if no reading is ready
 * @retval a HAL error or a ring buffer error otherwise; the errors counter
 *         counts these
 */
int zl_sensor_reader_sample(struct zl_sensor_reader *reader);

/** Number of failed readings since init. */
uint32_t zl_sensor_reader_errors(const struct zl_sensor_reader *reader);

#ifdef __cplusplus
}
#endif

#endif /* ZL_SENSOR_READER_H_ */
