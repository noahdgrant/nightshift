import java.util.List;

class Casts {
    @SuppressWarnings({ // both are needed
        "unchecked",
        "rawtypes"
    })
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
