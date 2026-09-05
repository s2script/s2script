#!/usr/bin/env bash
set -euo pipefail
# Fresh-process injection: never replace a domain whose native producers can still be alive.
export S2SCRIPT_ASYNC_LIMITS_JSON='{"jobs_global":2,"jobs_per_owner":2,"input_bytes":4096,"owner_input_bytes":2048,"input_item_bytes":1024,"completion_bytes":4096,"failure_bytes":256,"sockets_global":4,"sockets_per_owner":2,"sqlite_global":2,"sqlite_per_owner":1,"pools_global":2,"pools_per_owner":1,"socket_out_items":2,"socket_out_bytes":256,"outbound_bytes":512,"inbound_items":2,"inbound_bytes":512,"timers_global":4,"timers_per_owner":2,"worker_queue_items":2,"sqlite_queue_items":2,"http_body_bytes":256,"db_result_rows":2,"db_result_bytes":256,"cookie_versions":2,"cookie_bytes":1024,"cookie_write_bytes":512,"frame_items":1,"frame_bytes":128,"frame_poll_items":8}'
cargo test -p s2script-core v8host::frame_tests::async_tiny_policy_admission_reinit_and_frame_progress -- --ignored --exact
