//! 组织、部门、成员关系与管理员 scope 的 PostgreSQL 实现。

use async_trait::async_trait;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::organization::{
    AdminScope, Organization, OrganizationMembership, OrganizationUnit,
};
use tpl_agent_core::ports::DirectoryRepository;
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::is_foreign_key_violation;
use super::{column, is_unique_violation, storage_failure};

fn organization_from_row(row: &sqlx::postgres::PgRow) -> Result<Organization, AgentError> {
    Ok(Organization {
        id: column(row, "id")?,
        slug: column(row, "slug")?,
        display_name: column(row, "display_name")?,
        created_at: column(row, "created_at")?,
    })
}

fn unit_from_row(row: &sqlx::postgres::PgRow) -> Result<OrganizationUnit, AgentError> {
    Ok(OrganizationUnit {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        name: column(row, "name")?,
        created_at: column(row, "created_at")?,
    })
}

fn membership_from_row(row: &sqlx::postgres::PgRow) -> Result<OrganizationMembership, AgentError> {
    Ok(OrganizationMembership {
        organization_id: column(row, "organization_id")?,
        user_id: column(row, "user_id")?,
        unit_id: column(row, "unit_id")?,
        display_name: column(row, "display_name")?,
        created_at: column(row, "created_at")?,
    })
}

fn admin_scope_from_row(row: &sqlx::postgres::PgRow) -> Result<AdminScope, AgentError> {
    Ok(AdminScope {
        organization_id: column(row, "organization_id")?,
        admin_user_id: column(row, "admin_user_id")?,
        granted_by_admin_id: column(row, "granted_by_admin_id")?,
        created_at: column(row, "created_at")?,
    })
}

#[async_trait]
impl DirectoryRepository for PostgresAgentRepository {
    async fn create_organization(
        &self,
        organization: &Organization,
        created_by_admin_id: i64,
    ) -> Result<(), AgentError> {
        let insert_result = sqlx::query(
            "INSERT INTO organizations (id, slug, display_name, created_by_admin_id, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(organization.id)
        .bind(&organization.slug)
        .bind(&organization.display_name)
        .bind(created_by_admin_id)
        .bind(organization.created_at)
        .execute(&self.pool)
        .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error)
                if is_unique_violation(&insert_error, "organizations_slug_unique") =>
            {
                Err(AgentError::Conflict("organization_slug_taken"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn list_organizations(&self) -> Result<Vec<Organization>, AgentError> {
        let organization_rows = sqlx::query(
            "SELECT id, slug, display_name, created_at FROM organizations ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        organization_rows
            .iter()
            .map(organization_from_row)
            .collect()
    }

    async fn get_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Option<Organization>, AgentError> {
        let organization_row = sqlx::query(
            "SELECT id, slug, display_name, created_at FROM organizations WHERE id = $1",
        )
        .bind(organization_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        organization_row
            .as_ref()
            .map(organization_from_row)
            .transpose()
    }

    async fn find_organization_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<Organization>, AgentError> {
        let organization_row = sqlx::query(
            "SELECT id, slug, display_name, created_at FROM organizations WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        organization_row
            .as_ref()
            .map(organization_from_row)
            .transpose()
    }

    async fn create_unit(&self, unit: &OrganizationUnit) -> Result<(), AgentError> {
        let insert_result = sqlx::query(
            "INSERT INTO organization_units (id, organization_id, name, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(unit.id)
        .bind(unit.organization_id)
        .bind(&unit.name)
        .bind(unit.created_at)
        .execute(&self.pool)
        .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error)
                if is_unique_violation(&insert_error, "organization_units_name_unique") =>
            {
                Err(AgentError::Conflict("unit_name_taken"))
            }
            Err(insert_error)
                if is_foreign_key_violation(
                    &insert_error,
                    "organization_units_organization_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("organization_not_found"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn list_units(&self, organization_id: Uuid) -> Result<Vec<OrganizationUnit>, AgentError> {
        let unit_rows = sqlx::query(
            "SELECT id, organization_id, name, created_at FROM organization_units
             WHERE organization_id = $1 ORDER BY name",
        )
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        unit_rows.iter().map(unit_from_row).collect()
    }

    async fn delete_unit(&self, organization_id: Uuid, unit_id: Uuid) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let member_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM organization_memberships
                 WHERE organization_id = $1 AND unit_id = $2
             )",
        )
        .bind(organization_id)
        .bind(unit_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        if member_exists {
            return Err(AgentError::Conflict("unit_has_members"));
        }
        let delete_result =
            sqlx::query("DELETE FROM organization_units WHERE id = $1 AND organization_id = $2")
                .bind(unit_id)
                .bind(organization_id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_failure)?;
        if delete_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("unit_not_found"));
        }
        transaction.commit().await.map_err(storage_failure)
    }

    async fn upsert_membership(
        &self,
        membership: &OrganizationMembership,
    ) -> Result<(), AgentError> {
        let upsert_result = sqlx::query(
            "INSERT INTO organization_memberships
                 (organization_id, user_id, unit_id, display_name, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (organization_id, user_id) DO UPDATE
                 SET unit_id = EXCLUDED.unit_id,
                     display_name = EXCLUDED.display_name",
        )
        .bind(membership.organization_id)
        .bind(membership.user_id)
        .bind(membership.unit_id)
        .bind(&membership.display_name)
        .bind(membership.created_at)
        .execute(&self.pool)
        .await;
        match upsert_result {
            Ok(_) => Ok(()),
            Err(upsert_error)
                if is_foreign_key_violation(&upsert_error, "organization_memberships_unit_fk") =>
            {
                Err(AgentError::InvalidRequest("部门不属于当前组织".to_owned()))
            }
            Err(upsert_error)
                if is_foreign_key_violation(
                    &upsert_error,
                    "organization_memberships_organization_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("organization_not_found"))
            }
            Err(upsert_error) => Err(storage_failure(upsert_error)),
        }
    }

    async fn list_memberships(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<OrganizationMembership>, AgentError> {
        let membership_rows = sqlx::query(
            "SELECT organization_id, user_id, unit_id, display_name, created_at
             FROM organization_memberships WHERE organization_id = $1 ORDER BY user_id",
        )
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        membership_rows.iter().map(membership_from_row).collect()
    }

    async fn list_memberships_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<OrganizationMembership>, AgentError> {
        let membership_rows = sqlx::query(
            "SELECT organization_id, user_id, unit_id, display_name, created_at
             FROM organization_memberships WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        membership_rows.iter().map(membership_from_row).collect()
    }

    async fn get_membership(
        &self,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<Option<OrganizationMembership>, AgentError> {
        let membership_row = sqlx::query(
            "SELECT organization_id, user_id, unit_id, display_name, created_at
             FROM organization_memberships WHERE organization_id = $1 AND user_id = $2",
        )
        .bind(organization_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        membership_row.as_ref().map(membership_from_row).transpose()
    }

    async fn remove_membership(
        &self,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<(), AgentError> {
        sqlx::query(
            "DELETE FROM organization_memberships WHERE organization_id = $1 AND user_id = $2",
        )
        .bind(organization_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        Ok(())
    }

    async fn grant_admin_scope(&self, scope: &AdminScope) -> Result<(), AgentError> {
        let grant_result = sqlx::query(
            "INSERT INTO organization_admin_scopes
                 (organization_id, admin_user_id, granted_by_admin_id, created_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (organization_id, admin_user_id) DO UPDATE
                 SET granted_by_admin_id = EXCLUDED.granted_by_admin_id,
                     created_at = EXCLUDED.created_at",
        )
        .bind(scope.organization_id)
        .bind(scope.admin_user_id)
        .bind(scope.granted_by_admin_id)
        .bind(scope.created_at)
        .execute(&self.pool)
        .await;
        match grant_result {
            Ok(_) => Ok(()),
            Err(grant_error)
                if is_foreign_key_violation(
                    &grant_error,
                    "organization_admin_scopes_organization_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("organization_not_found"))
            }
            Err(grant_error) => Err(storage_failure(grant_error)),
        }
    }

    async fn list_admin_scopes(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<AdminScope>, AgentError> {
        let scope_rows = sqlx::query(
            "SELECT organization_id, admin_user_id, granted_by_admin_id, created_at
             FROM organization_admin_scopes WHERE organization_id = $1 ORDER BY admin_user_id",
        )
        .bind(organization_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        scope_rows.iter().map(admin_scope_from_row).collect()
    }

    async fn list_scopes_for_admin(
        &self,
        admin_user_id: i64,
    ) -> Result<Vec<AdminScope>, AgentError> {
        let scope_rows = sqlx::query(
            "SELECT organization_id, admin_user_id, granted_by_admin_id, created_at
             FROM organization_admin_scopes WHERE admin_user_id = $1 ORDER BY created_at",
        )
        .bind(admin_user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        scope_rows.iter().map(admin_scope_from_row).collect()
    }

    async fn get_admin_scope(
        &self,
        organization_id: Uuid,
        admin_user_id: i64,
    ) -> Result<Option<AdminScope>, AgentError> {
        let scope_row = sqlx::query(
            "SELECT organization_id, admin_user_id, granted_by_admin_id, created_at
             FROM organization_admin_scopes WHERE organization_id = $1 AND admin_user_id = $2",
        )
        .bind(organization_id)
        .bind(admin_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        scope_row.as_ref().map(admin_scope_from_row).transpose()
    }

    async fn revoke_admin_scope(
        &self,
        organization_id: Uuid,
        admin_user_id: i64,
    ) -> Result<(), AgentError> {
        sqlx::query(
            "DELETE FROM organization_admin_scopes
             WHERE organization_id = $1 AND admin_user_id = $2",
        )
        .bind(organization_id)
        .bind(admin_user_id)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        Ok(())
    }
}
