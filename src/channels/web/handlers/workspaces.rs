//! Shared workspace API handlers and scope resolution helpers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::channels::web::auth::{AuthenticatedUser, UserIdentity};
use crate::channels::web::server::GatewayState;
use crate::channels::web::types::*;
use crate::db::{Database, WorkspaceMembership, WorkspaceRecord};

pub const WORKSPACE_SCOPE_PREFIX: &str = "workspace:";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceQuery {
    pub workspace: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkspace {
    pub workspace: WorkspaceRecord,
    pub role: String,
}

pub fn workspace_scope_user_id(workspace_id: Uuid) -> String {
    format!("{WORKSPACE_SCOPE_PREFIX}{workspace_id}")
}

pub async fn resolve_workspace_scope(
    store: &Arc<dyn Database>,
    user: &UserIdentity,
    workspace_slug: Option<&str>,
) -> Result<Option<ResolvedWorkspace>, (StatusCode, String)> {
    let Some(slug) = workspace_slug else {
        return Ok(None);
    };

    let workspace = store
        .get_workspace_by_slug(slug)
        .await
        .map_err(internal_db_error)?
        .ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;

    if workspace.status == "archived" {
        return Err((StatusCode::GONE, "Workspace is archived".to_string()));
    }

    let role = store
        .get_member_role(workspace.id, &user.user_id)
        .await
        .map_err(internal_db_error)?
        .ok_or((StatusCode::FORBIDDEN, "Workspace access denied".to_string()))?;

    Ok(Some(ResolvedWorkspace { workspace, role }))
}

pub fn require_workspace_manager(role: &str) -> Result<(), (StatusCode, String)> {
    if matches!(role, "owner" | "admin") {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "Workspace admin or owner role required".to_string(),
        ))
    }
}

fn workspace_info_from_membership(membership: WorkspaceMembership) -> WorkspaceInfo {
    WorkspaceInfo {
        id: membership.workspace.id,
        name: membership.workspace.name,
        slug: membership.workspace.slug,
        description: membership.workspace.description,
        status: membership.workspace.status,
        role: membership.role,
        created_at: membership.workspace.created_at.to_rfc3339(),
        updated_at: membership.workspace.updated_at.to_rfc3339(),
        created_by: membership.workspace.created_by,
        settings: membership.workspace.settings,
    }
}

fn workspace_info(workspace: WorkspaceRecord, role: String) -> WorkspaceInfo {
    WorkspaceInfo {
        id: workspace.id,
        name: workspace.name,
        slug: workspace.slug,
        description: workspace.description,
        status: workspace.status,
        role,
        created_at: workspace.created_at.to_rfc3339(),
        updated_at: workspace.updated_at.to_rfc3339(),
        created_by: workspace.created_by,
        settings: workspace.settings,
    }
}

fn internal_db_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!("Workspace database error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal database error".to_string(),
    )
}

pub async fn workspaces_list_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<WorkspaceListResponse>, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;

    let workspaces = store
        .list_workspaces_for_user(&user.user_id)
        .await
        .map_err(internal_db_error)?
        .into_iter()
        .map(workspace_info_from_membership)
        .collect();

    Ok(Json(WorkspaceListResponse { workspaces }))
}

pub async fn workspaces_create_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Json(body): Json<WorkspaceCreateRequest>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;

    let workspace = store
        .create_workspace(
            &body.name,
            &body.slug,
            &body.description,
            &user.user_id,
            &body.settings,
        )
        .await
        .map_err(internal_db_error)?;

    Ok(Json(workspace_info(workspace, "owner".to_string())))
}

pub async fn workspaces_detail_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(slug): Path<String>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;
    Ok(Json(workspace_info(resolved.workspace, resolved.role)))
}

pub async fn workspaces_update_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(slug): Path<String>,
    Json(body): Json<WorkspaceUpdateRequest>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;
    require_workspace_manager(&resolved.role)?;

    let updated = store
        .update_workspace(
            resolved.workspace.id,
            &body.name,
            &body.description,
            &body.settings,
        )
        .await
        .map_err(internal_db_error)?
        .ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;

    Ok(Json(workspace_info(updated, resolved.role)))
}

pub async fn workspaces_archive_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(slug): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;
    require_workspace_manager(&resolved.role)?;

    let archived = store
        .archive_workspace(resolved.workspace.id)
        .await
        .map_err(internal_db_error)?;
    if archived {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "Workspace not found".to_string()))
    }
}

pub async fn workspace_members_list_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(slug): Path<String>,
) -> Result<Json<WorkspaceMembersResponse>, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;

    let members = store
        .list_workspace_members(resolved.workspace.id)
        .await
        .map_err(internal_db_error)?
        .into_iter()
        .map(|(user, membership)| WorkspaceMemberInfo {
            user_id: user.id,
            email: user.email,
            display_name: user.display_name,
            status: user.status,
            role: membership.role,
            joined_at: membership.joined_at.to_rfc3339(),
            invited_by: membership.invited_by,
        })
        .collect();

    Ok(Json(WorkspaceMembersResponse { members }))
}

pub async fn workspace_members_upsert_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path((slug, member_user_id)): Path<(String, String)>,
    Json(body): Json<WorkspaceMemberWriteRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;
    require_workspace_manager(&resolved.role)?;

    store
        .add_workspace_member(
            resolved.workspace.id,
            &member_user_id,
            &body.role,
            Some(&user.user_id),
        )
        .await
        .map_err(internal_db_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn workspace_members_delete_handler(
    State(state): State<Arc<GatewayState>>,
    AuthenticatedUser(user): AuthenticatedUser,
    Path((slug, member_user_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let store = state.store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available".to_string(),
    ))?;
    let resolved = resolve_workspace_scope(store, &user, Some(&slug)).await?;
    let resolved = resolved.ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;
    require_workspace_manager(&resolved.role)?;

    let deleted = store
        .remove_workspace_member(resolved.workspace.id, &member_user_id)
        .await
        .map_err(internal_db_error)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            "Workspace member not found".to_string(),
        ))
    }
}
