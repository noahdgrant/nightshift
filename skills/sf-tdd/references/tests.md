# Good and Bad Tests

## Good Tests

**Integration-style**: Test through real interfaces, not mocks of internal parts.

```python
# GOOD: Tests observable behavior
def test_user_can_checkout_with_valid_cart(product, payment_method):
    cart = Cart()
    cart.add(product)
    result = checkout(cart, payment_method)
    assert result.status == "confirmed"
```

Characteristics:

- Tests behavior users/callers care about
- Uses public API only
- Survives internal refactors
- Describes WHAT, not HOW
- One logical assertion per test

## Bad Tests

**Implementation-detail tests**: Coupled to internal structure.

```python
# BAD: Tests implementation details
def test_checkout_calls_payment_service_process(mocker, cart, payment):
    process = mocker.patch("shop.checkout.payment_service.process")
    checkout(cart, payment)
    process.assert_called_once_with(cart.total)
```

Red flags:

- Mocking internal collaborators
- Testing private methods
- Asserting on call counts/order
- Test breaks when refactoring without behavior change
- Test name describes HOW not WHAT
- Verifying through external means instead of interface

```python
# BAD: Bypasses interface to verify
def test_create_user_saves_to_database(db):
    create_user(name="Alice")
    row = db.execute("SELECT * FROM users WHERE name = ?", ("Alice",)).fetchone()
    assert row is not None


# GOOD: Verifies through interface
def test_create_user_makes_user_retrievable():
    user = create_user(name="Alice")
    retrieved = get_user(user.id)
    assert retrieved.name == "Alice"
```

**Tautological tests**: Expected value restates the implementation, so the test passes by construction.

```python
# BAD: Expected value is recomputed the way the code computes it
def test_calculate_total_sums_line_items():
    items = [Item(price=10), Item(price=5)]
    expected = sum(i.price for i in items)
    assert calculate_total(items) == expected


# GOOD: Expected value is an independent, known literal
def test_calculate_total_sums_line_items():
    assert calculate_total([Item(price=10), Item(price=5)]) == 15
```

**Firmware note.** Take expected values from the datasheet or the protocol spec, not from the driver's own constants. A test that asserts `encode_baud(115200) == BAUD_DIVISOR_115200` restates the code. A test that asserts the divisor the reference manual lists for your clock (`== 0x0008`) can disagree with it.
