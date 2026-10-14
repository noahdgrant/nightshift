class Runner {
    void run(Runnable task) {
        try {
            task.run();
        } catch (Throwable t) {
            throw new IllegalStateException(t);
        }
    }
}
