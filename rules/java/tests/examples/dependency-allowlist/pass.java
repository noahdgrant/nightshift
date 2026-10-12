import com.example.app.Store;
import java.util.List;
import org.junit.jupiter.api.Test;

class StoreTest {
    List<Store> stores = List.of();

    @Test
    void empty() {
        System.out.println(stores.isEmpty());
    }
}
