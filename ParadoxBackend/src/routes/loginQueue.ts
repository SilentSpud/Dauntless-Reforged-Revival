/*
 * [Reforged 0018] login-queue-prod.steelyard.ca: the login queue's maintenance status.
 *
 * The 1.14.7 client asks it once before it logs in (captured contract:
 * DauntlessEndpointDocumentation/Login/LoginQueue/MaintenanceStatus.md, no auth, response
 * { "show-status": false }). Unanswered, the 404 failed the client's
 * CheckCrossPlayAndCrossProgression login step, which ends on "An error occurred while communicating
 * with the game servers" and never reaches gamesession's /features/platform/win.
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { Router } from "express";

export const loginQueueRouter = Router();

loginQueueRouter.get("/maintenance/status", (req, res) => {
    res.json({ "show-status": false });
});
