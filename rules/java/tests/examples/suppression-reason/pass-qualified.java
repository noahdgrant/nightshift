import java.util.List;

class Casts {
    @java.lang.SuppressWarnings("unchecked") // the caller passes a List<String>
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
