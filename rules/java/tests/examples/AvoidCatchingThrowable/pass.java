class Runner {
    void run(Runnable task) {
        try {
            task.run();
        } catch (IllegalArgumentException e) {
            throw new IllegalStateException(e);
        }
    }
}
