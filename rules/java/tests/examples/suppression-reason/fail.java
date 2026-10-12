import java.util.List;

class Casts {
    @SuppressWarnings("unchecked")
    List<String> names(Object value) {
        return (List<String>) value;
    }
}
