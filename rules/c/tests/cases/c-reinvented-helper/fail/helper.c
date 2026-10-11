#define MIN(a, b) ((a) < (b) ? (a) : (b))

static int samples[8];

static int clamp_u8(int v)
{
	return v > 255 ? 255 : v;
}

int sample_count(void)
{
	return (int)(sizeof(samples) / sizeof(samples[0])) + clamp_u8(0);
}

int sample_count_ptr(const int *p)
{
	return (int)(sizeof(p) / sizeof(*p));
}

static int countof(const int *p)
{
	return p != 0;
}

#define container_of(ptr, type, member) ((type *)((char *)(ptr) - __builtin_offsetof(type, member)))

static int array_size(const int *p)
{
	return p != 0;
}

int bare_count(const int *p)
{
	int xs[4];
	int a = (int)(sizeof xs / sizeof xs[0]);
	int b = (int)(sizeof xs / sizeof *xs);
	return a + b + p[0];
}

static int my_max(int a, int b)
{
	return a > b ? a : b;
}

#define _MIN(a, b) ((a) < (b) ? (a) : (b))

static int max_int(int a, int b)
{
	return a > b ? a : b;
}

static float clamp_float(float v, float lo, float hi)
{
	return v < lo ? lo : (v > hi ? hi : v);
}

static int min_i16(int a, int b)
{
	return a < b ? a : b;
}

static unsigned max_u32(unsigned a, unsigned b)
{
	return a > b ? a : b;
}

static long clamp_i64(long v, long lo, long hi)
{
	return v < lo ? lo : (v > hi ? hi : v);
}

static long min_long(long a, long b)
{
	return a < b ? a : b;
}

static double max_double(double a, double b)
{
	return a > b ? a : b;
}
