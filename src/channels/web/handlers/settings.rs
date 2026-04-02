//! Settings API handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

// Setting keys used to persist hygiene config in the DB.
const HYGIENE_ENABLED_KEY: &str = "memory_hygiene.enabled";
const HYGIENE_DAILY_RETENTION_KEY: &str = "memory_hygiene.daily_retention_days";
const HYGIENE_CONVERSATION_RETENTION_KEY: &str = "memory_hygiene.conversation_retention_days";
const HYGIENE_CADENCE_KEY: &str = "memory_hygiene.cadence_hours";

use crate::channels::web::server::GatewayState;
use crate::channels::web::types::*;

pub async fn settings_list_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<SettingsListResponse>, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let rows = store.list_settings(&state.user_id).await.map_err(|e| {
        tracing::error!("Failed to list settings: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let settings = rows
        .into_iter()
        .map(|r| SettingResponse {
            key: r.key,
            value: r.value,
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(SettingsListResponse { settings }))
}

pub async fn settings_get_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> Result<Json<SettingResponse>, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let row = store
        .get_setting_full(&state.user_id, &key)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get setting '{}': {}", key, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(SettingResponse {
        key: row.key,
        value: row.value,
        updated_at: row.updated_at.to_rfc3339(),
    }))
}

pub async fn settings_set_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
    Json(body): Json<SettingWriteRequest>,
) -> Result<StatusCode, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    store
        .set_setting(&state.user_id, &key, &body.value)
        .await
        .map_err(|e| {
            tracing::error!("Failed to set setting '{}': {}", key, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn settings_delete_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    store
        .delete_setting(&state.user_id, &key)
        .await
        .map_err(|e| {
            tracing::error!("Failed to delete setting '{}': {}", key, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn settings_export_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<SettingsExportResponse>, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let settings = store.get_all_settings(&state.user_id).await.map_err(|e| {
        tracing::error!("Failed to export settings: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(SettingsExportResponse { settings }))
}

pub async fn settings_import_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SettingsImportRequest>,
) -> Result<StatusCode, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    store
        .set_all_settings(&state.user_id, &body.settings)
        .await
        .map_err(|e| {
            tracing::error!("Failed to import settings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/settings/hygiene — read current hygiene config from DB (falls back to defaults).
pub async fn settings_hygiene_get_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<HygieneSettingsResponse>, StatusCode> {
    let defaults = crate::config::HygieneConfig::default();
    let Some(store) = state.store.as_ref() else {
        return Ok(Json(HygieneSettingsResponse {
            enabled: defaults.enabled,
            daily_retention_days: defaults.daily_retention_days,
            conversation_retention_days: defaults.conversation_retention_days,
            cadence_hours: defaults.cadence_hours,
        }));
    };

    let get = |key: &'static str| {
        let store = store.clone();
        let user_id = state.user_id.clone();
        async move { store.get_setting(&user_id, key).await.ok().flatten() }
    };

    let enabled = get(HYGIENE_ENABLED_KEY)
        .await
        .and_then(|v| v.as_bool())
        .unwrap_or(defaults.enabled);
    let daily_retention_days = get(HYGIENE_DAILY_RETENTION_KEY)
        .await
        .and_then(|v| v.as_u64().map(|n| n as u32))
        .unwrap_or(defaults.daily_retention_days);
    let conversation_retention_days = get(HYGIENE_CONVERSATION_RETENTION_KEY)
        .await
        .and_then(|v| v.as_u64().map(|n| n as u32))
        .unwrap_or(defaults.conversation_retention_days);
    let cadence_hours = get(HYGIENE_CADENCE_KEY)
        .await
        .and_then(|v| v.as_u64().map(|n| n as u32))
        .unwrap_or(defaults.cadence_hours);

    Ok(Json(HygieneSettingsResponse {
        enabled,
        daily_retention_days,
        conversation_retention_days,
        cadence_hours,
    }))
}

/// PUT /api/settings/hygiene — update hygiene config in DB (partial update; omitted fields unchanged).
pub async fn settings_hygiene_set_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<HygieneSettingsRequest>,
) -> Result<StatusCode, StatusCode> {
    let store = state
        .store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    if let Some(v) = body.enabled {
        store
            .set_setting(&state.user_id, HYGIENE_ENABLED_KEY, &serde_json::Value::Bool(v))
            .await
            .map_err(|e| {
                tracing::error!("Failed to set hygiene.enabled: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }
    if let Some(v) = body.daily_retention_days {
        store
            .set_setting(
                &state.user_id,
                HYGIENE_DAILY_RETENTION_KEY,
                &serde_json::Value::Number(v.into()),
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to set hygiene.daily_retention_days: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }
    if let Some(v) = body.conversation_retention_days {
        store
            .set_setting(
                &state.user_id,
                HYGIENE_CONVERSATION_RETENTION_KEY,
                &serde_json::Value::Number(v.into()),
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to set hygiene.conversation_retention_days: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }
    if let Some(v) = body.cadence_hours {
        store
            .set_setting(
                &state.user_id,
                HYGIENE_CADENCE_KEY,
                &serde_json::Value::Number(v.into()),
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to set hygiene.cadence_hours: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    Ok(StatusCode::NO_CONTENT)
}
