## ADDED Requirements

### Requirement: Exclusive execution ownership during transfer
(P3) Every transferable Session SHALL have an authoritative owner and monotonically increasing ownership epoch. Transfer SHALL use durable states `preparing`, `ready`, `committed` and `aborted` with a unique transfer ID. The source SHALL quiesce tools and background mutations and persist its execution prohibition before sending state. The destination SHALL verify the manifest before the ownership authority atomically commits the new owner and epoch; it SHALL not execute earlier. Dispatch and hosted state writes SHALL require the current epoch. A source without confirmation SHALL remain paused and query the authority; a timeout SHALL NOT authorize local resume. A partitioned Runner SHALL stop new dispatch when its 30-second lease expires. Failover SHALL wait for prior lease expiry and reconcile in-flight external effects; fencing SHALL NOT be claimed to undo an already dispatched external action.

#### Scenario: Acknowledgement lost after handoff
- **WHEN** the destination becomes owner but the source loses the acknowledgement
- **THEN** the destination may execute, the source remains paused, and reconnect discovers the committed epoch without a second Drain
