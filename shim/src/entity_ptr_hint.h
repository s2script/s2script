// Pointer -> entity index hints, fed by the entity listener (entity_listener.cpp).
//
// Adopting a pointer the engine handed us (an engine-function return or hook argument declared as
// an entity) used to walk all MAX_TOTAL_ENTITIES identity slots looking for it. These hints let the
// caller check ONE slot first. A hint is never trusted on its own: the caller still validates the
// slot in the system-owned identity chunk (instance pointer, free bit, handle index), so the
// candidate pointer is never dereferenced, and on any miss it falls back to the full walk. A stale
// or missing hint therefore costs speed, never correctness.
#pragma once

// The index the listener last saw for `p`, or -1.
int S2EntityPtrHint(const void* p);
