#include <zephyr/sys/util.h>

#define MAX_RETRIES 5
#define MIN_SIZE 16

static int samples[8];
static int max_retries = MAX_RETRIES;

static int sample_limit(int v)
{
	return CLAMP(v, 0, 255);
}

static int minimum_gap(int a, int b)
{
	return a - b;
}

int sample_count(void)
{
	return (int)ARRAY_SIZE(samples) + sample_limit(0) + minimum_gap(1, 0) + max_retries + MIN_SIZE;
}

static int min_value(int a, int b)
{
	return a < b ? a : b;
}
