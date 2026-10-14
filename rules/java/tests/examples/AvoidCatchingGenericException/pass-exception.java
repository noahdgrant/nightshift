class Loader {
    Object load(java.util.concurrent.Callable<Object> task) {
        try {
            return task.call();
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }
}
