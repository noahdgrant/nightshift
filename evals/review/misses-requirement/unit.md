feat(cli): count prints the lines of a file, or of every file with --all

`tally count <file>` prints the number of lines in the file. `tally count --all <dir>` prints the number of lines of each file in the directory, one `<file> <n>` line each. A file that cannot be read exits 1 with a message naming it.
