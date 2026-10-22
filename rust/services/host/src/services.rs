//! The service supervisor (`legacy/ReforgedHost/src/services.ts`): starts each service with its env
//! file, waits until it answers, restarts it when it dies and stops the whole process tree (a
//! Windows job object, a Linux process group; RD3).
//!
//! Filled by RH2, for `start` only.
