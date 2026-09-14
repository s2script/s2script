// Host test for Metamod KHookImpl ownership and Unloader retirement.
//
// Compiles the actual provider (core/metamod_khook.h) and Unloader
// (unpatched: extracted from core/metamod_plugins.cpp; patched:
// core/metamod_khook_unloader.h). Physical removal is an injected backend
// (the KHOOK_STANDALONE KHook::* free functions). This is not a second hook
// engine and does not reimplement provider bookkeeping.
//
// Public IKHook ABI is unchanged. Receipt/Observe contracts stay in
// shim/src/khook_binding.h (R2).
#ifndef KHOOK_STANDALONE
#define KHOOK_STANDALONE
#endif

#include "metamod_khook.h"

#ifdef S2_KHOOK_HOST_PATCHED
#include "metamod_khook_unloader.h"
#else
#include "unpatched_unloader.h"
#endif

#include <cstdint>
#include <cstring>
#include <iostream>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>
#include <csignal>
#include <unistd.h>

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
	do {                                                                        \
		if (!(cond)) {                                                          \
			std::cerr << "FAIL: " << (msg) << "\n";                           \
			g_fail++;                                                           \
		} else {                                                                \
			std::cout << "ok:   " << (msg) << "\n";                            \
		}                                                                       \
		std::cout.flush();                                                       \
		std::cerr.flush();                                                       \
	} while (0)

// ---------------------------------------------------------------------------
// Injected physical-removal backend (pinned KHook absent-id semantics).
// ---------------------------------------------------------------------------
namespace {

thread_local std::vector<void*> g_ctx_stack;

struct BackendHook {
	void* context = nullptr;
	void* removed_function = nullptr;
	enum State { PendingInsert, Associated, Removing, Gone } state = PendingInsert;
	bool hold_original = false;
};

struct RemovalReq {
	KHook::HookID_t id = KHook::INVALID_HOOK;
	void (*fn)(KHook::HookID_t, void*) = nullptr;
	void* ctx = nullptr;
};

struct PhysicalBackend {
	KHook::HookID_t next_id = 1;
	int setup_hook_calls = 0;
	int setup_virtual_calls = 0;
	int remove_calls = 0;
	std::unordered_map<KHook::HookID_t, BackendHook> hooks;
	std::vector<RemovalReq> queued;

	void reset() {
		next_id = 1;
		setup_hook_calls = 0;
		setup_virtual_calls = 0;
		remove_calls = 0;
		hooks.clear();
		queued.clear();
	}

	KHook::HookID_t insert(void* context, void* removed, bool virtual_hook) {
		if (virtual_hook) {
			++setup_virtual_calls;
		} else {
			++setup_hook_calls;
		}
		const KHook::HookID_t id = next_id++;
		hooks[id] = BackendHook{context, removed, BackendHook::PendingInsert, false};
		return id;
	}

	void associate(KHook::HookID_t id) {
		auto it = hooks.find(id);
		if (it != hooks.end() && it->second.state == BackendHook::PendingInsert) {
			it->second.state = BackendHook::Associated;
		}
	}

	void hold_original(KHook::HookID_t id, bool hold) {
		auto it = hooks.find(id);
		if (it != hooks.end()) {
			it->second.hold_original = hold;
		}
	}

	void remove(KHook::HookID_t id, bool async, void (*fn)(KHook::HookID_t, void*), void* ctx) {
		++remove_calls;
		auto it = hooks.find(id);
		if (it == hooks.end() || it->second.state == BackendHook::Gone) {
			// Pinned KHook async absent-id path: return without completion.
			(void)async;
			return;
		}
		if (it->second.state == BackendHook::PendingInsert) {
			BackendHook h = it->second;
			hooks.erase(it);
			g_ctx_stack.push_back(h.context);
			if (h.removed_function) {
				auto helper = reinterpret_cast<void (*)(KHook::HookID_t)>(h.removed_function);
				helper(id);
			}
			g_ctx_stack.pop_back();
			if (fn) {
				fn(id, ctx);
			}
			return;
		}
		it->second.state = BackendHook::Removing;
		queued.push_back(RemovalReq{id, fn, ctx});
	}

	bool complete(KHook::HookID_t id) {
		auto it = hooks.find(id);
		if (it == hooks.end()) {
			return false;
		}
		if (it->second.hold_original) {
			return false;
		}
		BackendHook h = it->second;
		it->second.state = BackendHook::Gone;
		g_ctx_stack.push_back(h.context);
		if (h.removed_function) {
			auto helper = reinterpret_cast<void (*)(KHook::HookID_t)>(h.removed_function);
			helper(id);
		}
		g_ctx_stack.pop_back();
		std::vector<RemovalReq> fire;
		for (auto qit = queued.begin(); qit != queued.end();) {
			if (qit->id == id) {
				fire.push_back(*qit);
				qit = queued.erase(qit);
			} else {
				++qit;
			}
		}
		hooks.erase(id);
		for (const auto& req : fire) {
			if (req.fn) {
				req.fn(req.id, req.ctx);
			}
		}
		return true;
	}
};

PhysicalBackend g_backend;

}  // namespace

namespace KHook {

HookID_t SetupHook(void*, void* context, void* removed_function, void*, void*, void*, void*,
                   unsigned int, bool) {
	return g_backend.insert(context, removed_function, false);
}

HookID_t SetupVirtualHook(void**, int, void* context, void* removed_function, void*, void*, void*,
                          void*, unsigned int, bool) {
	return g_backend.insert(context, removed_function, true);
}

void RemoveHook(HookID_t id, bool async, void (*hook_removal_fn)(HookID_t, void*), void* context) {
	g_backend.remove(id, async, hook_removal_fn, context);
}

void* GetContextPtr() {
	return g_ctx_stack.empty() ? nullptr : g_ctx_stack.back();
}

void* GetOriginalFunction() {
	return nullptr;
}

void* GetOriginalValuePtr() {
	return nullptr;
}

void* GetOverrideValuePtr() {
	return nullptr;
}

void* GetCurrentValuePtr(bool) {
	return nullptr;
}

void DestroyReturnValue() {}

void* FindOriginal(void*) {
	return nullptr;
}

void* FindOriginalVirtual(void**, int) {
	return nullptr;
}

void* DoRecall(Action, void*, std::size_t, void*, void*) {
	return nullptr;
}

void SaveReturnValue(Action, void*, std::size_t, void*, void*, bool) {}

void* LookupSignature(void*, std::size_t, const char*) {
	return nullptr;
}

bool WasOriginalFunctionSkipped() {
	return false;
}

}  // namespace KHook

// ---------------------------------------------------------------------------
// Library-release intercept (--wrap=dlclose). Provider must stay valid
// through this call.
// ---------------------------------------------------------------------------
static int g_release_count = 0;
static void* g_released_handle = nullptr;
static void (*g_on_dlclose)() = nullptr;

extern "C" int __wrap_dlclose(void* handle) {
	++g_release_count;
	g_released_handle = handle;
	if (g_on_dlclose) {
		g_on_dlclose();
	}
	return 0;
}

static void* FakeLib() {
	return reinterpret_cast<void*>(static_cast<uintptr_t>(0x4B1000));
}

struct Session {
#ifdef S2_KHOOK_HOST_PATCHED
	std::shared_ptr<KHookImpl> owned;
	std::weak_ptr<KHookImpl> weak;
#else
	KHookImpl* owned = nullptr;
#endif
	KHook::IKHook* api = nullptr;
	void* lib = nullptr;

	Session() {
		lib = FakeLib();
#ifdef S2_KHOOK_HOST_PATCHED
		owned = std::make_shared<KHookImpl>();
		weak = owned;
		api = owned.get();
#else
		owned = new KHookImpl();
		api = owned;
#endif
	}

	void close() {
#ifdef S2_KHOOK_HOST_PATCHED
		auto* unloader = new Unloader(lib, std::move(owned));
		unloader->Check();
#else
		auto hooks = std::move(owned->m_hooks);
		auto* unloader = new Unloader(lib, std::move(hooks));
		unloader->Check();
		delete owned;
		owned = nullptr;
#endif
	}

	bool provider_live() const {
#ifdef S2_KHOOK_HOST_PATCHED
		return !weak.expired();
#else
		return owned != nullptr;
#endif
	}
};

static KHook::IKHook* g_api = nullptr;
static int g_helper_count = 0;
static void* g_helper_ctx = nullptr;
static int g_complete_count = 0;
static KHook::HookID_t g_complete_id = KHook::INVALID_HOOK;
static void* g_dummy_ctx = reinterpret_cast<void*>(static_cast<uintptr_t>(0xC0));

static void HelperRemoved(KHook::HookID_t) {
	++g_helper_count;
	g_helper_ctx = g_api ? g_api->GetContextPtr() : nullptr;
}

static void OnComplete(KHook::HookID_t id, void* ctx) {
	++g_complete_count;
	g_complete_id = id;
	(void)ctx;
}

static void reset_counts() {
	g_helper_count = 0;
	g_helper_ctx = nullptr;
	g_complete_count = 0;
	g_complete_id = KHook::INVALID_HOOK;
	g_release_count = 0;
	g_released_handle = nullptr;
	g_on_dlclose = nullptr;
	g_backend.reset();
	g_ctx_stack.clear();
}

static KHook::HookID_t setup_inline(KHook::IKHook* api, void* ctx = g_dummy_ctx) {
	return api->SetupHook(reinterpret_cast<void*>(static_cast<uintptr_t>(0x10)), ctx,
	                      reinterpret_cast<void*>(&HelperRemoved), nullptr, nullptr, nullptr,
	                      nullptr, 16u, true);
}

static KHook::HookID_t setup_virtual(KHook::IKHook* api, void* ctx = g_dummy_ctx) {
	static void* vtable[4] = {nullptr, nullptr, nullptr, nullptr};
	return api->SetupVirtualHook(vtable, 1, ctx, reinterpret_cast<void*>(&HelperRemoved), nullptr,
	                             nullptr, nullptr, nullptr, 16u, true);
}

static void test_inline_and_virtual() {
	reset_counts();
	Session s;
	g_api = s.api;
	const auto id_fn = setup_inline(s.api);
	const auto id_vt = setup_virtual(s.api);
	CHECK(id_fn != KHook::INVALID_HOOK, "inline_and_virtual: SetupHook accepted");
	CHECK(id_vt != KHook::INVALID_HOOK, "inline_and_virtual: SetupVirtualHook accepted");
	CHECK(id_fn != id_vt, "inline_and_virtual: distinct ids");
#ifndef S2_KHOOK_HOST_PATCHED
	CHECK(s.owned->m_hooks.count(id_fn) == 1,
	      "inline_and_virtual: inline id belongs to the provider");
	CHECK(s.owned->m_hooks.count(id_vt) == 1,
	      "inline_and_virtual: virtual id belongs to the provider");
#endif
	g_backend.associate(id_fn);
	g_backend.associate(id_vt);
	const int removes_before = g_backend.remove_calls;
	s.close();
	CHECK(g_backend.remove_calls - removes_before == 2,
	      "inline_and_virtual: host retire issues one backend remove per accepted id");
	CHECK(g_backend.complete(id_fn), "inline_and_virtual: inline removal can complete");
	CHECK(g_backend.complete(id_vt), "inline_and_virtual: virtual removal can complete");
	CHECK(g_release_count == 1, "inline_and_virtual: library released after both ids complete");
}

static void test_unknown_id() {
	reset_counts();
	Session s;
	g_api = s.api;
	const int backend_before = g_backend.remove_calls;
	s.api->RemoveHook(0xFFFFu, true, &OnComplete, nullptr);
	CHECK(g_complete_count == 1, "unknown_id: absent async removal completes");
	CHECK(g_backend.remove_calls == backend_before,
	      "unknown_id: absent async removal does not call the backend");
	s.close();
	CHECK(g_release_count == 1, "unknown_id: idle close still releases the library");
}

static void test_duplicate_remove() {
	reset_counts();
	Session s;
	g_api = s.api;
	const auto id = setup_virtual(s.api);
	g_backend.associate(id);
	int n = 0;
	auto bump = [](KHook::HookID_t, void* ctx) {
		++*static_cast<int*>(ctx);
	};
	s.api->RemoveHook(id, true, bump, &n);
	s.api->RemoveHook(id, true, bump, &n);
	s.api->RemoveHook(id, true, bump, &n);
	CHECK(g_backend.remove_calls == 1, "duplicate_remove: one backend call for N subscribers");
	CHECK(g_backend.complete(id), "duplicate_remove: backend completes the id");
	CHECK(n == 3, "duplicate_remove: N completions");
	s.close();
	CHECK(g_release_count == 1, "duplicate_remove: unload after explicit join does not hang");
}

static void test_explicit_then_unload() {
	reset_counts();
	Session s;
	g_api = s.api;
	const auto id = setup_virtual(s.api);
	g_backend.associate(id);
	s.api->RemoveHook(id, true, &OnComplete, nullptr);
	CHECK(g_backend.complete(id), "explicit_then_unload: explicit removal completes");
	CHECK(g_complete_count == 1, "explicit_then_unload: explicit completion delivered");
	const int backend_after_explicit = g_backend.remove_calls;
	s.close();
	CHECK(g_backend.remove_calls == backend_after_explicit,
	      "explicit_then_unload: unload does not re-issue the absent-id backend path");
	CHECK(g_release_count == 1, "explicit_then_unload: no stale-id hang; library released");
}

static void test_pending_insert() {
	reset_counts();
	Session s;
	g_api = s.api;
	void* ctx = reinterpret_cast<void*>(static_cast<uintptr_t>(0x51));
	const auto id = setup_inline(s.api, ctx);
	CHECK(g_backend.hooks.count(id) == 1 &&
	          g_backend.hooks[id].state == BackendHook::PendingInsert,
	      "pending_insert: id is queued before first fire");
	s.api->RemoveHook(id, true, &OnComplete, nullptr);
	CHECK(g_helper_count == 1, "pending_insert: helper exactly once");
	CHECK(g_helper_ctx == ctx, "pending_insert: helper GetContext sees live context");
	CHECK(g_complete_count == 1, "pending_insert: completion exactly once");
	CHECK(g_backend.remove_calls == 1, "pending_insert: one backend remove");
	s.api->RemoveHook(id, true, &OnComplete, nullptr);
	CHECK(g_helper_count == 1, "pending_insert: second remove does not re-run helper");
	CHECK(g_complete_count == 2, "pending_insert: second remove completes without a second helper");
	CHECK(g_backend.remove_calls == 1,
	      "pending_insert: second remove does not call the backend absent-id path");
	s.close();
	CHECK(g_release_count == 1, "pending_insert: library released after cancellation");
}

static void test_provider_during_post() {
	reset_counts();
	Session s;
	g_api = s.api;
	KHook::IKHook* api = s.api;
	void* ctx = reinterpret_cast<void*>(static_cast<uintptr_t>(0x70));
	const auto id = setup_virtual(s.api, ctx);
	g_backend.associate(id);
	g_backend.hold_original(id, true);
	g_ctx_stack.push_back(ctx);
	s.close();
	CHECK(api->GetContextPtr() == ctx, "provider_during_post: GetContextPtr usable in POST");
	CHECK(api->GetOriginalFunction() == nullptr,
	      "provider_during_post: GetOriginalFunction usable in POST");
	CHECK(api->WasOriginalFunctionSkipped() == false,
	      "provider_during_post: WasOriginalFunctionSkipped usable in POST");
	const auto rejected =
	    api->SetupHook(reinterpret_cast<void*>(static_cast<uintptr_t>(0x11)), ctx, nullptr, nullptr,
	                   nullptr, nullptr, nullptr, 16u, true);
	CHECK(rejected == KHook::INVALID_HOOK,
	      "provider_during_post: registration refused after host retire");
#ifdef S2_KHOOK_HOST_PATCHED
	CHECK(s.provider_live(), "provider_during_post: shared owner still live during original");
#endif
	g_ctx_stack.pop_back();
	g_backend.hold_original(id, false);
	CHECK(g_backend.complete(id), "provider_during_post: removal completes after original");
	CHECK(g_release_count == 1, "provider_during_post: library released after POST");
#ifdef S2_KHOOK_HOST_PATCHED
	CHECK(!s.provider_live(), "provider_during_post: provider released after idle");
#endif
}

static void test_provider_during_remove() {
	reset_counts();
	Session s;
	g_api = s.api;
	void* ctx = reinterpret_cast<void*>(static_cast<uintptr_t>(0x71));
	const auto id = setup_virtual(s.api, ctx);
	g_backend.associate(id);
	s.close();
	CHECK(g_backend.complete(id), "provider_during_remove: backend completes");
	CHECK(g_helper_count == 1, "provider_during_remove: helper ran");
	CHECK(g_helper_ctx == ctx, "provider_during_remove: helper GetContext via live API provider");
	CHECK(g_release_count == 1, "provider_during_remove: library released after helper");
}

static int g_dlclose_api_ok = 0;

static void test_provider_during_dlclose() {
	reset_counts();
	Session s;
	g_api = s.api;
	KHook::IKHook* api = s.api;
	g_dlclose_api_ok = 0;
	g_on_dlclose = []() {
		if (g_api) {
			(void)g_api->GetContextPtr();
			(void)g_api->GetOriginalFunction();
			(void)g_api->WasOriginalFunctionSkipped();
			g_dlclose_api_ok = 1;
		}
	};
	const auto id = setup_virtual(s.api);
	g_backend.associate(id);
	s.close();
	CHECK(g_backend.complete(id), "provider_during_dlclose: removal completes into dlclose");
	CHECK(g_dlclose_api_ok == 1,
	      "provider_during_dlclose: static destruction calls provider; valid until close returns");
	CHECK(g_release_count == 1, "provider_during_dlclose: library released once");
	(void)api;
}

static void test_reentrant_completion() {
	reset_counts();
	alarm(8);
	Session s;
	g_api = s.api;
	const auto id1 = setup_virtual(s.api, reinterpret_cast<void*>(static_cast<uintptr_t>(0x81)));
	const auto id2 = setup_virtual(s.api, reinterpret_cast<void*>(static_cast<uintptr_t>(0x82)));
	g_backend.associate(id1);
	g_backend.associate(id2);
	int inner = 0;
	struct Reenter {
		KHook::IKHook* api;
		KHook::HookID_t other;
		int* inner;
	} re{s.api, id2, &inner};
	auto outer = [](KHook::HookID_t, void* ctx) {
		auto* r = static_cast<Reenter*>(ctx);
		r->api->RemoveHook(r->other, true,
		                    [](KHook::HookID_t, void* ic) { ++*static_cast<int*>(ic); }, r->inner);
	};
	s.api->RemoveHook(id1, true, outer, &re);
	CHECK(g_backend.complete(id1), "reentrant_completion: first id completed");
	CHECK(g_backend.remove_calls == 2,
	      "reentrant_completion: inner RemoveHook issued the second backend remove");
	CHECK(g_backend.complete(id2), "reentrant_completion: second id completed");
	CHECK(inner == 1, "reentrant_completion: inner completion delivered (no lock inversion)");
	s.close();
	CHECK(g_release_count == 1, "reentrant_completion: library released");
	alarm(0);
}

static void test_release_once() {
	reset_counts();
	Session s;
	g_api = s.api;
	s.close();
	CHECK(g_release_count == 1,
	      "release_once: no in-flight work releases the library once");
	CHECK(g_release_count != 0, "release_once: no permanent library leak");
#ifdef S2_KHOOK_HOST_PATCHED
	CHECK(!s.provider_live(), "release_once: provider released (no permanent provider leak)");
#endif
}

using CaseFn = void (*)();

struct NamedCase {
	const char* name;
	CaseFn fn;
};

static const NamedCase kCases[] = {
    {"provider_during_post", &test_provider_during_post},
    {"provider_during_remove", &test_provider_during_remove},
    {"provider_during_dlclose", &test_provider_during_dlclose},
    {"inline_and_virtual", &test_inline_and_virtual},
    {"explicit_then_unload", &test_explicit_then_unload},
    {"duplicate_remove", &test_duplicate_remove},
    {"pending_insert", &test_pending_insert},
    {"unknown_id", &test_unknown_id},
    {"reentrant_completion", &test_reentrant_completion},
    {"release_once", &test_release_once},
};

static int run_one(const char* name) {
	for (const auto& c : kCases) {
		if (std::strcmp(c.name, name) == 0) {
			std::cout << "CASE " << name << " START\n";
			c.fn();
			if (g_fail == 0) {
				std::cout << "CASE " << name << " PASS\n";
				return 0;
			}
			std::cerr << "CASE " << name << " FAIL count=" << g_fail << "\n";
			return 1;
		}
	}
	std::cerr << "FAIL: unknown case " << name << "\n";
	return 2;
}

int main(int argc, char** argv) {
	if (argc >= 2) {
		return run_one(argv[1]);
	}
	int failed_cases = 0;
	for (const auto& c : kCases) {
		g_fail = 0;
		std::cout << "CASE " << c.name << " START\n";
		c.fn();
		if (g_fail) {
			std::cerr << "CASE " << c.name << " FAIL count=" << g_fail << "\n";
			failed_cases++;
		} else {
			std::cout << "CASE " << c.name << " PASS\n";
		}
	}
	if (failed_cases) {
		std::cerr << "host lifetime: " << failed_cases << " case(s) failed\n";
		return 1;
	}
	std::cout << "host lifetime: all cases passed\n";
	return 0;
}
