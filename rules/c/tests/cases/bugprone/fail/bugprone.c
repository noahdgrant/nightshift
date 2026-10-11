#include <string.h>

void copy_name(char *dst, const char *src)
{
	memcpy(dst, src, strlen(src));
}
