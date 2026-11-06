//! Runtime、Agent、访问规则、入口/委派策略与 Skill 的 PostgreSQL 实现。

use async_trait::async_trait;
use serde_json::Value;
use sqlx::postgres::PgRow;
use time::OffsetDateTime;
use tpl_agent_core::agent_catalog::{AccessEffect, AccessSubject, AgentAccessRule, BusinessAgent};
use tpl_agent_core::delegation::DelegationPolicy;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::{CatalogRepository, StoredSkillAsset};
use tpl_agent_core::routing::EntryPolicy;
use tpl_agent_core::runtime_catalog::{
    BuiltinRuntimeDeclaration, RuntimeAdapterKind, RuntimeConnection, RuntimeOrigin, RuntimeStatus,
};
use tpl_agent_core::skill_access::{Skill, SkillAsset};
use tpl_agent_core::time_utils::now;
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::{
    column, deserialize_column, is_foreign_key_violation, is_unique_violation, parse_database_enum,
    serialize_structure, storage_failure, uuid_array_json, uuids_from_json,
};

const RUNTIME_COLUMNS: &str = "id, organization_id, name, display_name, origin, adapter_kind, \
     endpoint_url, credential_alias, status, status_detail, config, created_at, updated_at";
const AGENT_COLUMNS: &str = "id, organization_id, runtime_connection_id, name, display_name, \
     description, prompt, owner_admin_id, enabled, descriptor_checksum, created_at, updated_at";

fn runtime_from_row(row: &PgRow) -> Result<RuntimeConnection, AgentError> {
    let origin_text: String = column(row, "origin")?;
    let adapter_text: String = column(row, "adapter_kind")?;
    let status_text: String = column(row, "status")?;
    let config_value: Value = column(row, "config")?;
    Ok(RuntimeConnection {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        name: column(row, "name")?,
        display_name: column(row, "display_name")?,
        origin: parse_database_enum(&origin_text, RuntimeOrigin::from_database)?,
        adapter_kind: parse_database_enum(&adapter_text, RuntimeAdapterKind::from_database)?,
        endpoint_url: column(row, "endpoint_url")?,
        credential_alias: column(row, "credential_alias")?,
        status: parse_database_enum(&status_text, RuntimeStatus::from_database)?,
        status_detail: column(row, "status_detail")?,
        config: deserialize_column(config_value, "runtime_config")?,
        created_at: column(row, "created_at")?,
        updated_at: column(row, "updated_at")?,
    })
}

fn agent_from_row(row: &PgRow) -> Result<BusinessAgent, AgentError> {
    Ok(BusinessAgent {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        runtime_connection_id: column(row, "runtime_connection_id")?,
        name: column(row, "name")?,
        display_name: column(row, "display_name")?,
        description: column(row, "description")?,
        prompt: column(row, "prompt")?,
        owner_admin_id: column(row, "owner_admin_id")?,
        enabled: column(row, "enabled")?,
        descriptor_checksum: column(row, "descriptor_checksum")?,
        created_at: column(row, "created_at")?,
        updated_at: column(row, "updated_at")?,
    })
}

fn access_subject_from_columns(
    subject_kind: &str,
    subject_id: &str,
) -> Result<AccessSubject, AgentError> {
    match subject_kind {
        "organization" => subject_id
            .parse::<Uuid>()
            .map(AccessSubject::Organization)
            .map_err(|_| AgentError::StorageUnavailable),
        "unit" => subject_id
            .parse::<Uuid>()
            .map(AccessSubject::Unit)
            .map_err(|_| AgentError::StorageUnavailable),
        "user" => subject_id
            .parse::<i64>()
            .map(AccessSubject::User)
            .map_err(|_| AgentError::StorageUnavailable),
        _ => Err(AgentError::StorageUnavailable),
    }
}

fn access_rule_from_row(row: &PgRow) -> Result<AgentAccessRule, AgentError> {
    let effect_text: String = column(row, "effect")?;
    let subject_kind: String = column(row, "subject_kind")?;
    let subject_id: String = column(row, "subject_id")?;
    Ok(AgentAccessRule {
        id: column(row, "id")?,
        agent_id: column(row, "agent_id")?,
        effect: parse_database_enum(&effect_text, AccessEffect::from_database)?,
        subject: access_subject_from_columns(&subject_kind, &subject_id)?,
        created_at: column(row, "created_at")?,
    })
}

fn entry_policy_from_row(row: &PgRow) -> Result<EntryPolicy, AgentError> {
    Ok(EntryPolicy {
        agent_id: column(row, "agent_id")?,
        organization_id: column(row, "organization_id")?,
        auto_visible: column(row, "auto_visible")?,
        priority: column(row, "priority")?,
        updated_at: column(row, "updated_at")?,
    })
}

fn delegation_policy_from_row(row: &PgRow) -> Result<DelegationPolicy, AgentError> {
    let child_agent_values: Value = column(row, "child_agent_ids")?;
    Ok(DelegationPolicy {
        parent_agent_id: column(row, "parent_agent_id")?,
        child_agent_ids: uuids_from_json(&child_agent_values)?,
        max_depth: u8::try_from(column::<i32>(row, "max_depth")?)
            .map_err(|_| AgentError::StorageUnavailable)?,
        max_child_runs: u8::try_from(column::<i32>(row, "max_child_runs")?)
            .map_err(|_| AgentError::StorageUnavailable)?,
    })
}

fn skill_from_row(row: &PgRow) -> Result<Skill, AgentError> {
    Ok(Skill {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        name: column(row, "name")?,
        description: column(row, "description")?,
        instructions: column(row, "instructions")?,
        enabled: column(row, "enabled")?,
        created_at: column(row, "created_at")?,
        updated_at: column(row, "updated_at")?,
    })
}

fn skill_asset_from_row(row: &PgRow) -> Result<StoredSkillAsset, AgentError> {
    Ok(StoredSkillAsset {
        asset: SkillAsset {
            id: column(row, "id")?,
            skill_id: column(row, "skill_id")?,
            name: column(row, "name")?,
            media_type: column(row, "media_type")?,
            size_bytes: column(row, "size_bytes")?,
            sha256: column(row, "sha256")?,
        },
        storage_key: column(row, "storage_key")?,
    })
}

fn runtime_insert_binds<'query>(
    query: sqlx::query::Query<'query, sqlx::Postgres, sqlx::postgres::PgArguments>,
    runtime: &RuntimeConnection,
) -> sqlx::query::Query<'query, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(runtime.id)
        .bind(runtime.organization_id)
        .bind(&runtime.name)
        .bind(&runtime.display_name)
        .bind(runtime.origin.as_str())
        .bind(runtime.adapter_kind.as_str())
        .bind(&runtime.endpoint_url)
        .bind(&runtime.credential_alias)
        .bind(runtime.status.as_str())
        .bind(&runtime.status_detail)
}

#[async_trait]
impl CatalogRepository for PostgresAgentRepository {
    async fn sync_builtin_runtimes(
        &self,
        declarations: &[BuiltinRuntimeDeclaration],
    ) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        for declaration in declarations {
            let existing_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM runtime_connections WHERE organization_id IS NULL AND name = $1",
            )
            .bind(&declaration.name)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_failure)?;
            let current_time = now();
            let runtime = declaration.to_connection(
                existing_id.unwrap_or_else(Uuid::new_v4),
                current_time,
                current_time,
            );
            let runtime_config = serialize_structure(&runtime.config)?;
            if existing_id.is_some() {
                sqlx::query(
                    "UPDATE runtime_connections
                     SET display_name = $2, adapter_kind = $3, status = $4, status_detail = $5,
                         config = $6, updated_at = $7
                     WHERE id = $1",
                )
                .bind(runtime.id)
                .bind(&runtime.display_name)
                .bind(runtime.adapter_kind.as_str())
                .bind(runtime.status.as_str())
                .bind(&runtime.status_detail)
                .bind(runtime_config)
                .bind(runtime.updated_at)
                .execute(&mut *transaction)
                .await
                .map_err(storage_failure)?;
            } else {
                let insert_query = runtime_insert_binds(
                    sqlx::query(
                        "INSERT INTO runtime_connections
                             (id, organization_id, name, display_name, origin, adapter_kind,
                              endpoint_url, credential_alias, status, status_detail,
                              config, created_at, updated_at)
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
                    ),
                    &runtime,
                );
                insert_query
                    .bind(runtime_config)
                    .bind(runtime.created_at)
                    .bind(runtime.updated_at)
                    .execute(&mut *transaction)
                    .await
                    .map_err(storage_failure)?;
            }
        }
        transaction.commit().await.map_err(storage_failure)
    }

    async fn insert_external_runtime(&self, runtime: &RuntimeConnection) -> Result<(), AgentError> {
        let runtime_config = serialize_structure(&runtime.config)?;
        let insert_query = runtime_insert_binds(
            sqlx::query(
                "INSERT INTO runtime_connections
                     (id, organization_id, name, display_name, origin, adapter_kind,
                      endpoint_url, credential_alias, status, status_detail,
                      config, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            ),
            runtime,
        );
        let insert_result = insert_query
            .bind(runtime_config)
            .bind(runtime.created_at)
            .bind(runtime.updated_at)
            .execute(&self.pool)
            .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error)
                if is_unique_violation(
                    &insert_error,
                    "runtime_connections_name_unique_external",
                ) =>
            {
                Err(AgentError::Conflict("runtime_name_taken"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn update_runtime(&self, runtime: &RuntimeConnection) -> Result<(), AgentError> {
        let runtime_config = serialize_structure(&runtime.config)?;
        let update_result = sqlx::query(
            "UPDATE runtime_connections
             SET display_name = $2, endpoint_url = $3, credential_alias = $4, status = $5,
                 status_detail = $6, config = $7, updated_at = $8
             WHERE id = $1",
        )
        .bind(runtime.id)
        .bind(&runtime.display_name)
        .bind(&runtime.endpoint_url)
        .bind(&runtime.credential_alias)
        .bind(runtime.status.as_str())
        .bind(&runtime.status_detail)
        .bind(runtime_config)
        .bind(runtime.updated_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        if update_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("runtime_not_found"));
        }
        Ok(())
    }

    async fn get_runtime(&self, runtime_id: Uuid) -> Result<Option<RuntimeConnection>, AgentError> {
        let runtime_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtime_connections WHERE id = $1"
        )))
        .bind(runtime_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        runtime_row.as_ref().map(runtime_from_row).transpose()
    }

    async fn find_runtime_by_name(
        &self,
        organization_id: Option<Uuid>,
        name: &str,
    ) -> Result<Option<RuntimeConnection>, AgentError> {
        let runtime_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtime_connections
             WHERE name = $2 AND organization_id IS NOT DISTINCT FROM $1
             LIMIT 1"
        )))
        .bind(organization_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        runtime_row.as_ref().map(runtime_from_row).transpose()
    }

    async fn list_runtimes_for_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<RuntimeConnection>, AgentError> {
        let runtime_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtime_connections
             WHERE organization_id IS NULL OR organization_id = $1
             ORDER BY (organization_id IS NOT NULL), name"
        )))
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        runtime_rows.iter().map(runtime_from_row).collect()
    }

    async fn insert_agent(
        &self,
        agent: &BusinessAgent,
        rules: &[AgentAccessRule],
    ) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let insert_result = sqlx::query(
            "INSERT INTO business_agents
                 (id, organization_id, runtime_connection_id, name, display_name, description,
                  prompt, owner_admin_id, enabled, descriptor_checksum, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(agent.id)
        .bind(agent.organization_id)
        .bind(agent.runtime_connection_id)
        .bind(&agent.name)
        .bind(&agent.display_name)
        .bind(&agent.description)
        .bind(&agent.prompt)
        .bind(agent.owner_admin_id)
        .bind(agent.enabled)
        .bind(&agent.descriptor_checksum)
        .bind(agent.created_at)
        .bind(agent.updated_at)
        .execute(&mut *transaction)
        .await;
        match insert_result {
            Ok(_) => {}
            Err(insert_error)
                if is_unique_violation(&insert_error, "business_agents_name_unique") =>
            {
                return Err(AgentError::Conflict("agent_name_taken"));
            }
            Err(insert_error)
                if is_foreign_key_violation(
                    &insert_error,
                    "business_agents_runtime_connection_id_fkey",
                ) =>
            {
                return Err(AgentError::RuntimeUnavailable);
            }
            Err(insert_error) => return Err(storage_failure(insert_error)),
        }
        for rule in rules {
            sqlx::query(
                "INSERT INTO agent_access_rules
                     (id, agent_id, effect, subject_kind, subject_id, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(rule.id)
            .bind(rule.agent_id)
            .bind(rule.effect.as_str())
            .bind(rule.subject.kind_str())
            .bind(rule.subject.subject_id())
            .bind(rule.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        }
        transaction.commit().await.map_err(storage_failure)
    }

    async fn update_agent(&self, agent: &BusinessAgent) -> Result<(), AgentError> {
        let update_result = sqlx::query(
            "UPDATE business_agents
             SET display_name = $2, description = $3, prompt = $4, enabled = $5,
                 descriptor_checksum = $6, updated_at = $7
             WHERE id = $1",
        )
        .bind(agent.id)
        .bind(&agent.display_name)
        .bind(&agent.description)
        .bind(&agent.prompt)
        .bind(agent.enabled)
        .bind(&agent.descriptor_checksum)
        .bind(agent.updated_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        if update_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("agent_not_found"));
        }
        Ok(())
    }

    async fn get_agent(&self, agent_id: Uuid) -> Result<Option<BusinessAgent>, AgentError> {
        let agent_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {AGENT_COLUMNS} FROM business_agents WHERE id = $1"
        )))
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        agent_row.as_ref().map(agent_from_row).transpose()
    }

    async fn list_agents(&self, organization_id: Uuid) -> Result<Vec<BusinessAgent>, AgentError> {
        let agent_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {AGENT_COLUMNS} FROM business_agents WHERE organization_id = $1 ORDER BY name"
        )))
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        agent_rows.iter().map(agent_from_row).collect()
    }

    async fn replace_access_rules(
        &self,
        agent_id: Uuid,
        rules: &[AgentAccessRule],
    ) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        sqlx::query("DELETE FROM agent_access_rules WHERE agent_id = $1")
            .bind(agent_id)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        for rule in rules {
            let insert_result = sqlx::query(
                "INSERT INTO agent_access_rules
                     (id, agent_id, effect, subject_kind, subject_id, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(rule.id)
            .bind(agent_id)
            .bind(rule.effect.as_str())
            .bind(rule.subject.kind_str())
            .bind(rule.subject.subject_id())
            .bind(rule.created_at)
            .execute(&mut *transaction)
            .await;
            if let Err(insert_error) = insert_result {
                return Err(
                    if is_unique_violation(&insert_error, "agent_access_rules_unique") {
                        AgentError::Conflict("access_rule_conflict")
                    } else {
                        storage_failure(insert_error)
                    },
                );
            }
        }
        transaction.commit().await.map_err(storage_failure)
    }

    async fn list_access_rules(&self, agent_id: Uuid) -> Result<Vec<AgentAccessRule>, AgentError> {
        let rule_rows = sqlx::query(
            "SELECT id, agent_id, effect, subject_kind, subject_id, created_at
             FROM agent_access_rules WHERE agent_id = $1 ORDER BY created_at, id",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        rule_rows.iter().map(access_rule_from_row).collect()
    }

    async fn get_entry_policy(&self, agent_id: Uuid) -> Result<Option<EntryPolicy>, AgentError> {
        let policy_row = sqlx::query(
            "SELECT agent_id, organization_id, auto_visible, priority, updated_at
             FROM agent_entry_policies WHERE agent_id = $1",
        )
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        policy_row.as_ref().map(entry_policy_from_row).transpose()
    }

    async fn upsert_entry_policy(&self, policy: &EntryPolicy) -> Result<(), AgentError> {
        let upsert_result = sqlx::query(
            "INSERT INTO agent_entry_policies
                 (agent_id, organization_id, auto_visible, priority, updated_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (agent_id) DO UPDATE
                 SET auto_visible = EXCLUDED.auto_visible,
                     priority = EXCLUDED.priority,
                     updated_at = EXCLUDED.updated_at",
        )
        .bind(policy.agent_id)
        .bind(policy.organization_id)
        .bind(policy.auto_visible)
        .bind(policy.priority)
        .bind(policy.updated_at)
        .execute(&self.pool)
        .await;
        match upsert_result {
            Ok(_) => Ok(()),
            Err(upsert_error)
                if is_foreign_key_violation(
                    &upsert_error,
                    "agent_entry_policies_agent_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("agent_not_found"))
            }
            Err(upsert_error) => Err(storage_failure(upsert_error)),
        }
    }

    async fn list_entry_policies(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<EntryPolicy>, AgentError> {
        let policy_rows = sqlx::query(
            "SELECT agent_id, organization_id, auto_visible, priority, updated_at
             FROM agent_entry_policies WHERE organization_id = $1 ORDER BY priority, agent_id",
        )
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        policy_rows.iter().map(entry_policy_from_row).collect()
    }

    async fn get_delegation_policy(
        &self,
        parent_agent_id: Uuid,
    ) -> Result<Option<DelegationPolicy>, AgentError> {
        let policy_row = sqlx::query(
            "SELECT parent_agent_id, child_agent_ids, max_depth, max_child_runs
             FROM agent_delegation_policies WHERE parent_agent_id = $1",
        )
        .bind(parent_agent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        policy_row
            .as_ref()
            .map(delegation_policy_from_row)
            .transpose()
    }

    async fn upsert_delegation_policy(&self, policy: &DelegationPolicy) -> Result<(), AgentError> {
        let child_agent_ids = uuid_array_json(&policy.child_agent_ids);
        let upsert_result = sqlx::query(
            "INSERT INTO agent_delegation_policies
                 (parent_agent_id, child_agent_ids, max_depth, max_child_runs, updated_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (parent_agent_id) DO UPDATE
                 SET child_agent_ids = EXCLUDED.child_agent_ids,
                     max_depth = EXCLUDED.max_depth,
                     max_child_runs = EXCLUDED.max_child_runs,
                     updated_at = EXCLUDED.updated_at",
        )
        .bind(policy.parent_agent_id)
        .bind(child_agent_ids)
        .bind(i32::from(policy.max_depth))
        .bind(i32::from(policy.max_child_runs))
        .bind(now())
        .execute(&self.pool)
        .await;
        match upsert_result {
            Ok(_) => Ok(()),
            Err(upsert_error)
                if is_foreign_key_violation(
                    &upsert_error,
                    "agent_delegation_policies_parent_agent_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("agent_not_found"))
            }
            Err(upsert_error) => Err(storage_failure(upsert_error)),
        }
    }

    async fn insert_skill(&self, skill: &Skill) -> Result<(), AgentError> {
        let insert_result = sqlx::query(
            "INSERT INTO agent_skills
                 (id, organization_id, name, description, instructions, enabled, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(skill.id)
        .bind(skill.organization_id)
        .bind(&skill.name)
        .bind(&skill.description)
        .bind(&skill.instructions)
        .bind(skill.enabled)
        .bind(skill.created_at)
        .bind(skill.updated_at)
        .execute(&self.pool)
        .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error) if is_unique_violation(&insert_error, "agent_skills_name_unique") => {
                Err(AgentError::Conflict("skill_name_taken"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn update_skill(&self, skill: &Skill) -> Result<(), AgentError> {
        let update_result = sqlx::query(
            "UPDATE agent_skills
             SET description = $2, instructions = $3, enabled = $4, updated_at = $5
             WHERE id = $1",
        )
        .bind(skill.id)
        .bind(&skill.description)
        .bind(&skill.instructions)
        .bind(skill.enabled)
        .bind(skill.updated_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        if update_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("skill_not_found"));
        }
        Ok(())
    }

    async fn get_skill(&self, skill_id: Uuid) -> Result<Option<Skill>, AgentError> {
        let skill_row = sqlx::query(
            "SELECT id, organization_id, name, description, instructions, enabled,
                    created_at, updated_at
             FROM agent_skills WHERE id = $1",
        )
        .bind(skill_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        skill_row.as_ref().map(skill_from_row).transpose()
    }

    async fn list_skills(&self, organization_id: Uuid) -> Result<Vec<Skill>, AgentError> {
        let skill_rows = sqlx::query(
            "SELECT id, organization_id, name, description, instructions, enabled,
                    created_at, updated_at
             FROM agent_skills WHERE organization_id = $1 ORDER BY name",
        )
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        skill_rows.iter().map(skill_from_row).collect()
    }

    async fn list_skill_assignments(&self, agent_id: Uuid) -> Result<Vec<Uuid>, AgentError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT skill_id FROM agent_skill_assignments WHERE agent_id = $1 ORDER BY skill_id",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)
    }

    async fn replace_skill_assignments(
        &self,
        agent_id: Uuid,
        skill_ids: &[Uuid],
    ) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        sqlx::query("DELETE FROM agent_skill_assignments WHERE agent_id = $1")
            .bind(agent_id)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        let created_at: OffsetDateTime = now();
        for skill_id in skill_ids {
            sqlx::query(
                "INSERT INTO agent_skill_assignments (agent_id, skill_id, created_at)
                 VALUES ($1, $2, $3)",
            )
            .bind(agent_id)
            .bind(skill_id)
            .bind(created_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        }
        transaction.commit().await.map_err(storage_failure)
    }

    async fn insert_skill_asset(
        &self,
        asset: &SkillAsset,
        storage_key: &str,
    ) -> Result<(), AgentError> {
        let insert_result = sqlx::query(
            "INSERT INTO agent_skill_assets
                 (id, skill_id, name, media_type, size_bytes, sha256, storage_key, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(asset.id)
        .bind(asset.skill_id)
        .bind(&asset.name)
        .bind(&asset.media_type)
        .bind(asset.size_bytes)
        .bind(&asset.sha256)
        .bind(storage_key)
        .bind(now())
        .execute(&self.pool)
        .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error)
                if is_unique_violation(&insert_error, "agent_skill_assets_name_unique") =>
            {
                Err(AgentError::Conflict("skill_asset_name_taken"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn list_skill_assets(
        &self,
        skill_ids: &[Uuid],
    ) -> Result<Vec<StoredSkillAsset>, AgentError> {
        if skill_ids.is_empty() {
            return Ok(Vec::new());
        }
        let asset_rows = sqlx::query(
            "SELECT id, skill_id, name, media_type, size_bytes, sha256, storage_key
             FROM agent_skill_assets
             WHERE skill_id = ANY($1)
             ORDER BY skill_id, name",
        )
        .bind(skill_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        asset_rows.iter().map(skill_asset_from_row).collect()
    }

    async fn get_skill_asset(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<StoredSkillAsset>, AgentError> {
        let asset_row = sqlx::query(
            "SELECT id, skill_id, name, media_type, size_bytes, sha256, storage_key
             FROM agent_skill_assets WHERE id = $1",
        )
        .bind(asset_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        asset_row.as_ref().map(skill_asset_from_row).transpose()
    }

    async fn delete_skill_asset(&self, asset_id: Uuid) -> Result<(), AgentError> {
        sqlx::query("DELETE FROM agent_skill_assets WHERE id = $1")
            .bind(asset_id)
            .execute(&self.pool)
            .await
            .map_err(storage_failure)?;
        Ok(())
    }
}
