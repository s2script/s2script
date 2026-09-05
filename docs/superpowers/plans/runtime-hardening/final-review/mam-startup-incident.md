# Server-side addon startup incident — unresolved

The final 60-minute soak passed on September 5, before this separate human-test setup attempt. No production source or installed core/shim binaries changed during the incident.

The owned test server initially used `mm_extra_addons ""` and `mm_client_extra_addons "3790153369"`. After archiving the passing soak, the operator saved the exact MAM config, moved the addon ID to `mm_extra_addons`, cleared `mm_client_extra_addons`, and restarted only `s2script-cs2-hardening`.

MAM 1.5.4 downloaded and mounted the addon successfully. Metamod reported s2script loaded; V8 initialization, game-package registration and plugin-directory setup completed. All archives were readable as the container's steam user, the loader thread existed, and mounted binary hashes matched the accepted release. However, no TypeScript plugins activated and `sm`/`sm_clive_status` were unknown. This narrows the missing progress to the frame-driven loader path, but does not establish which component caused it.

A same-map `changelevel de_inferno` was used to compare with the earlier working boot's second map activation. It caused a segmentation fault around `2026-09-05T18:39:06Z`. No human was connected. The server process exited; the container had no automatic restart policy. Breakpad captured `7aaa8e4a-2684-4869-fcb1a984-61a5679a.dmp` and its binary `.s2meta` sidecar.

Offline parsing using the repository's Breakpad layouts identified thread 86, SIGSEGV (11), SEGV_MAPERR (1), and both instruction pointer and fault address zero. There is no containing module for address zero. The first saved return-address candidate is `0x7e79896af9d8`, within CS2 `libserver.so` at image-relative offset `0x22af9d8`. This is consistent with a null code-pointer call or jump, but is not a symbolicated stack and does not identify which component supplied the pointer. The sidecar's `core/idle` breadcrumb likewise does not establish causation.

Recovery restored the original MAM config byte-for-byte and cold-started the owned container. All 19 plugins then ran: the original 18 plus the temporary s2bench viewer fixture. Both fixture commands respond. The server uses its original client-side addon delivery for the pending human test. This is a rollback to a working configuration, **not a fix or root-cause attribution**. Server-side addon mounting and this map-transition path remain unaccepted.

The private dump, sidecar and full logs are preserved under `/home/ghirakawa/s2script-hardening/.gate/final-ba6c7c1/human-map-crash` and the corresponding local hardening evidence directory. Raw process memory is not committed to the repository. The failed and original MAM configs are retained separately in the remote staging directory. Production `s2script-hudlab` was not changed.
