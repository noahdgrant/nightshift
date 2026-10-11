#pragma GCC diagnostic ignored "-Wcast-qual" /* the vendor header casts away const */

int legacy_value(void) // NOLINT(readability-x): kept for the bootloader ABI
{
	return 0;
}

/* NOLINTNEXTLINE(bugprone-branch-clone): both arms read different registers */
int other_value(void);

// NOLINTBEGIN(bugprone-x): generated tables
int table_value(void);
// NOLINTEND(bugprone-x)

int fourth_value(void); // cppcheck-suppress unusedFunction // the linker script calls it

#pragma warning(disable : 4996) // the vendor header uses strcpy

// the generated table no longer needs its old NOLINT
