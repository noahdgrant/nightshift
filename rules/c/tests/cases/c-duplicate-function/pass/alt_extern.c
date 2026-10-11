#ifdef CONFIG_FAST_CHOOSE
int choose(int v)
{
	return v;
}
#else
int choose(int v)
{
	return v + 1;
}
#endif
