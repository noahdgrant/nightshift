class Closer {
    int close() {
        try {
            return 1;
        } finally {
            System.out.println("closed");
        }
    }
}
