feat(cli): greet takes an optional --shout flag

`tally greet <name> --shout` prints the greeting in upper case, `HELLO, ADA!`. Without the flag the greeting is unchanged. With no name it still exits 2 and prints a usage line to stderr.
