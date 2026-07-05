//! Application workflows (sagas).
//!
//! No cross-entity sagas are defined for POS yet. The retail recognition saga
//! (billing + payment orchestration) lives in `PosWriteService::recognize_sale`,
//! not here. Add saga modules under this directory when a real flow needs one.
