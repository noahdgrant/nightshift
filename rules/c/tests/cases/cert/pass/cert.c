#include <stdlib.h>

long parse_rate(const char *text, char **end)
{
	return strtol(text, end, 10);
}
