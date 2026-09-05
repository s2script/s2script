---
"@s2script/sdk": minor
---

Expose WebSocket.send, TcpSocket.send, and UdpSocket.sendTo acceptance as boolean returns. False means closed/oversized/full admission and no accepted send. Correct threadSleep's declaration to its existing Promise<void> runtime result, replacing obsolete fiber documentation. Document named async and timer overload errors and finite query/HTTP result limits. Existing callers may continue ignoring returns; structural mocks that returned void must return the new value. Host apiVersion 2 remains compatible.
