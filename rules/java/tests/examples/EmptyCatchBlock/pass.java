class Parser {
    int parse(String text) {
        try {
            return Integer.parseInt(text);
        } catch (NumberFormatException e) {
            return -1;
        }
    }
}
