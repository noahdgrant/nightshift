import java.util.List;

class Casts {
    @SuppressWarnings({
        "unchecked",
        "rawtypes"
    })
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
