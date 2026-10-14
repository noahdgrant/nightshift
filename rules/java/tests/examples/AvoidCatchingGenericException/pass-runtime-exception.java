class Guard {
    void run(Runnable task) {
        try {
            task.run();
        } catch (RuntimeException e) {
            throw new IllegalStateException(e);
        }
    }
}
