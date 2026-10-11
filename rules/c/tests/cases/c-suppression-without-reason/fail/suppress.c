#pragma GCC diagnostic ignored "-Wcast-qual"

int legacy_value(void) // NOLINT
{
	return 0;
}

/* NOLINTNEXTLINE(bugprone-branch-clone) */
int other_value(void);

// cppcheck-suppress unusedFunction
int third_value(void);

// NOLINTBEGIN
int table_value(void);
// NOLINTEND

#pragma clang diagnostic ignored "-Wcast-qual"
#pragma warning(disable : 4996)
