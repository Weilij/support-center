//! Teams (CRD §3.2, lines 1792-2154): team CRUD, statistics, multi-team member
//! management, member account lifecycle, agent-team associations, per-team QR codes.
//!
//! `POST /api/teams/members/{memberId}/reset` lives in `crate::domain::auth` (CRD
//! 2013-2018 matches the §1.1 behavior already implemented there), and the self-service
//! password change is mounted at `/api/auth/change-password` (CRD 2020).

pub mod handlers;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Health/info probes and the QR test endpoint are explicitly unauthenticated
    // (CRD 1814-1821, 2127).
    let public = Router::new()
        .route("/api/teams/health", get(handlers::health))
        .route("/api/teams/info", get(handlers::info))
        .route("/api/teams/{id}/qr-code-test", post(handlers::qr_code_test));

    let authed = Router::new()
        .route("/api/teams", get(handlers::list_teams).post(handlers::create_team))
        .route("/api/teams/transfer", post(handlers::transfer_agents))
        .route("/api/teams/stats/all", get(handlers::all_team_stats))
        .route("/api/teams/search/{query}", get(handlers::search_teams))
        // Member-account family (CRD 1933-2009).
        .route(
            "/api/teams/members",
            get(handlers::list_all_members).post(handlers::create_member),
        )
        .route("/api/teams/members/check-email", get(handlers::check_email))
        .route("/api/teams/members/bulk-delete", post(handlers::bulk_delete_members))
        .route("/api/teams/members/bulk-update", post(handlers::bulk_update_members))
        .route("/api/teams/members/batch-edit", post(handlers::batch_edit_members))
        .route("/api/teams/members/batch-edit/undo", post(handlers::undo_batch_edit))
        .route(
            "/api/teams/members/{memberId}",
            put(handlers::update_member_account).delete(handlers::delete_member_account),
        )
        .route("/api/teams/members/{memberId}/status", put(handlers::set_member_status))
        .route("/api/teams/members/{memberId}/role", put(handlers::set_member_role))
        // Agent-team association family (CRD 2029-2074).
        .route("/api/teams/agent-teams/{agentId}", get(handlers::agent_teams))
        .route(
            "/api/teams/agent-teams/team/{teamId}/members",
            get(handlers::team_members_detail),
        )
        .route("/api/teams/agent-teams/{agentId}/join", post(handlers::join_team))
        .route(
            "/api/teams/agent-teams/{agentId}/join-multiple",
            post(handlers::join_multiple),
        )
        .route(
            "/api/teams/agent-teams/{agentId}/leave/{teamId}",
            delete(handlers::leave_team),
        )
        .route(
            "/api/teams/agent-teams/{agentId}/role/{teamId}",
            put(handlers::update_membership_role),
        )
        .route(
            "/api/teams/agent-teams/{agentId}/primary/{teamId}",
            put(handlers::set_primary_team),
        )
        // Team-scoped family (CRD 1830-1929) and QR family (CRD 2078-2124).
        .route(
            "/api/teams/{id}",
            get(handlers::get_team).put(handlers::update_team).delete(handlers::delete_team),
        )
        .route("/api/teams/{id}/stats", get(handlers::team_stats))
        .route(
            "/api/teams/{id}/members",
            get(handlers::team_members).post(handlers::add_member),
        )
        .route("/api/teams/{id}/members/batch", post(handlers::batch_add_members))
        .route("/api/teams/{id}/members/bulk-remove", post(handlers::bulk_remove_members))
        .route(
            "/api/teams/{id}/members/{agentId}",
            put(handlers::update_team_member).delete(handlers::remove_team_member),
        )
        .route("/api/teams/{id}/qr-code", post(handlers::generate_qr))
        .route("/api/teams/{id}/qr-codes", get(handlers::list_qr_codes))
        .route("/api/teams/{id}/qr-code/latest", get(handlers::latest_qr))
        .route("/api/teams/{id}/qr-code/fast", get(handlers::fast_qr))
        .route(
            "/api/teams/{id}/qr-code/liff",
            get(handlers::get_liff_qr).post(handlers::generate_liff_qr),
        )
        .route("/api/teams/{id}/qr-code/liff/stats", get(handlers::liff_qr_stats))
        .route(
            "/api/teams/{id}/qr-codes/{qrCodeId}/deactivate",
            put(handlers::deactivate_qr),
        )
        .layer(from_fn_with_state(state.clone(), require_auth));

    public.merge(authed)
}
