#ifdef CONFIG_FAST_PICK
static int pick(int v)
{
	return v;
}
#else
static int pick(int v)
{
	return v + 1;
}
#endif

int picked(int v)
{
	return pick(v);
}
