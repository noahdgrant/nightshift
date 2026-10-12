import java.util.ArrayList;
import java.util.List;

final class Lists {
    @SafeVarargs
    public static <T> List<T> listOf(T... items) {
        List<T> out = new ArrayList<>();
        for (T item : items) {
            out.add(item);
        }
        return out;
    }
}
