import static java.util.Objects.requireNonNull;

class Checked {
    Object value(Object v) {
        return requireNonNull(v);
    }
}
