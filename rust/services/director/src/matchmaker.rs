//! `HandleMatchmakingRequest` (`src/controllers/matchmaker.ts`): which server a player goes to.
//! CITY is Ramsgate; SHARED the Training Grounds or a public hunt; ISLAND a hunt from travel
//! arguments or from a hunt id. Any failure stays a failure: redirecting to Ramsgate would make the
//! client claim a hunt was found.

use std::sync::Arc;

use serde_json::Value;
use service_kit::js;

use crate::gameservers::{Director, StartError, TRAINING_DOJO_HUNT_ID, length, nonblank};
use crate::log;

pub async fn handle(director: &Arc<Director>, body: Option<&Value>) -> Result<Value, StartError> {
    // `req.body.GameMode` of an absent body throws.
    let body =
        body.ok_or_else(|| StartError::Other("Cannot read properties of undefined (reading 'GameMode')".into()))?;
    let game_mode = body.get("GameMode");
    let game_args = body.get("GameArgs");
    let hunt_id = body.get("HuntId");
    let expected = body.get("ExpectedPlayers");
    log::info(&format!(
        "Handling matchmaking with GameMode: {} HuntId: {} and GameArgs: {}",
        js::string(game_mode),
        js::string(hunt_id),
        js::string(game_args)
    ));
    match game_mode.and_then(Value::as_str) {
        Some("CITY") => return director.ramsgate_connection_details(),
        Some("SHARED") => {
            if let Some(hunt) = nonblank(hunt_id).map_err(StartError::Other)? {
                if hunt == TRAINING_DOJO_HUNT_ID {
                    return director.training_connection_details_for_request().await;
                }
                // Normal Hunting Grounds arrive as SHARED too (2026-07-13).
                if expected.is_some_and(|e| !e.is_null()) && length(expected).is_some_and(|n| n > 0) {
                    if let Some(reused) = director.try_reuse_shared_hunt_server(hunt, expected) {
                        return Ok(reused);
                    }
                    let expected = expected.cloned().unwrap_or(Value::Null);
                    return director
                        .startup_with_hunt_id(hunt, &expected)
                        .await
                        .inspect_err(|error| {
                            log::error(&format!("SHARED hunt '{hunt}' failed to resolve/start: {error}"));
                        });
                }
                log::error(&format!(
                    "SHARED hunt '{hunt}' had no ExpectedPlayers; cannot start a hunt server"
                ));
                return Err(StartError::Other(format!(
                    "SHARED hunt '{hunt}' had no ExpectedPlayers"
                )));
            }
        }
        Some("ISLAND") => {
            let island = async {
                if let Some(args) = nonblank(game_args).map_err(StartError::Other)? {
                    return Some(director.startup_with_args(args, hunt_id, expected).await).transpose();
                }
                if let Some(hunt) = nonblank(hunt_id).map_err(StartError::Other)?
                    && expected.is_some_and(|e| !e.is_null())
                {
                    let expected = expected.cloned().unwrap_or(Value::Null);
                    return Some(director.startup_with_hunt_id(hunt, &expected).await).transpose();
                }
                Ok(None)
            };
            match island.await {
                Ok(Some(details)) => return Ok(details),
                Ok(None) => {}
                Err(error) => {
                    // A missing 1.12 row, a full cap or a failed readiness check must stay visible.
                    log::error(&format!(
                        "ISLAND hunt (huntId='{}') could not start: {error}",
                        js::string(hunt_id)
                    ));
                    return Err(error);
                }
            }
        }
        _ => {}
    }
    let (mode, hunt) = (js::string(game_mode), js::string(hunt_id));
    log::error(&format!(
        "Matchmaking failed: unsupported request GameMode='{mode}' HuntId='{hunt}'"
    ));
    Err(StartError::Other(format!(
        "Unsupported matchmaking request GameMode='{mode}' HuntId='{hunt}'"
    )))
}
