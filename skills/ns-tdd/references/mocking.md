# When to Mock

Mock at **system boundaries** only:

- External APIs (payment, email, etc.)
- Databases (sometimes - prefer test DB)
- Time/randomness
- File system (sometimes)
- Hardware: registers, peripherals, buses (firmware, through the hardware abstraction layer)

Don't mock:

- Your own classes/modules
- Internal collaborators
- Anything you control

## Designing for Mockability

At system boundaries, design interfaces that are easy to mock:

**1. Use dependency injection**

Pass external dependencies in rather than creating them internally:

```python
# Easy to mock
def process_payment(order, payment_client):
    return payment_client.charge(order.total)


# Hard to mock
def process_payment(order):
    client = StripeClient(os.environ["STRIPE_KEY"])
    return client.charge(order.total)
```

**2. Prefer SDK-style interfaces over generic fetchers**

Create specific functions for each external operation instead of one generic function with conditional logic:

```python
# GOOD: Each method is independently fakeable
class ShopApi:
    def get_user(self, user_id): return self._http.get(f"/users/{user_id}")
    def get_orders(self, user_id): return self._http.get(f"/users/{user_id}/orders")
    def create_order(self, data): return self._http.post("/orders", json=data)


# BAD: Faking requires conditional logic inside the fake
class ShopApi:
    def fetch(self, endpoint, **options): return self._http.request(endpoint, **options)
```

The SDK approach means:
- Each fake returns one specific shape
- No conditional logic in test setup
- Easier to see which endpoints a test exercises
- Type safety per endpoint (a `Protocol` per boundary)

**Firmware note.** The hardware abstraction layer is the SDK-style interface for the board: one function per operation (`uart_write`, `gpio_set`, `adc_read`), injected into the driver. A host test passes a fake HAL that records writes and returns scripted reads. Assert on what the driver produces through its own interface (the decoded frame, the state it reports), and reserve assertions on the fake's recorded writes for when the bytes on the wire are the behavior, as in a protocol encoder.
