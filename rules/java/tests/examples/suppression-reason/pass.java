import java.util.List;

class Casts {
    @SuppressWarnings("unchecked") // the caller passes a List<String>
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
