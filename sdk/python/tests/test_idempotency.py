import uuid

from bastion_control_plane import generate_idempotency_key


def test_generates_a_valid_uuid():
    key = generate_idempotency_key()
    # Raises ValueError if not a valid UUID string -- the assertion itself.
    uuid.UUID(key)


def test_generates_distinct_keys_across_calls():
    a = generate_idempotency_key()
    b = generate_idempotency_key()
    assert a != b
