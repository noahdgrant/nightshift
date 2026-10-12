import java.util.List;

class Casts {
    @SuppressWarnings("unchecked")
    // the reason belongs on the line above
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
