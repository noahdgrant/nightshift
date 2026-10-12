import java.util.List;

class Casts {
    @SuppressWarnings({
        "unchecked",
        "rawtypes"
    }) // the caller passes a List<String>
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
