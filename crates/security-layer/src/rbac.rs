//! Role-Based Access Control (RBAC) authorization engine.
//!
//! Defines platform permissions and maps them to roles (Admin, Operator).
//! The `authorize` function checks whether a given role has the required
//! permission, returning `SecurityError::InsufficientPermissions` on denial.

use common::errors::SecurityError;
use common::models::Role;

// =============================================================================
// Permission Definitions
// =============================================================================

/// All discrete permissions available on the platform.
///
/// Each permission maps to a specific category of actions a user can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Create, update, or delete user accounts.
    ManageUsers,
    /// Assign or revoke roles.
    ManageRoles,
    /// Modify system-wide configuration (LLM providers, scheduler settings, etc.).
    ManageConfig,
    /// Modify security settings (sandbox rules, rate limits, lockout policies).
    ManageSecurity,
    /// Create, cancel, or modify scheduled tasks.
    ExecuteTasks,
    /// View system and task execution logs.
    ViewLogs,
    /// Send messages through messaging integrations.
    SendMessages,
    /// View agent and system status.
    ViewStatus,
}

impl Permission {
    /// Returns the minimum role required to hold this permission.
    /// Used in error messages when authorization is denied.
    pub fn required_role(&self) -> Role {
        match self {
            Permission::ManageUsers
            | Permission::ManageRoles
            | Permission::ManageConfig
            | Permission::ManageSecurity => Role::Admin,
            Permission::ExecuteTasks
            | Permission::ViewLogs
            | Permission::SendMessages
            | Permission::ViewStatus => Role::Operator,
        }
    }
}

// =============================================================================
// Role Permission Sets
// =============================================================================

/// Returns the set of permissions granted to the given role.
pub fn permissions_for_role(role: &Role) -> &'static [Permission] {
    match role {
        Role::Admin => &[
            Permission::ManageUsers,
            Permission::ManageRoles,
            Permission::ManageConfig,
            Permission::ManageSecurity,
            Permission::ExecuteTasks,
            Permission::ViewLogs,
            Permission::SendMessages,
            Permission::ViewStatus,
        ],
        Role::Operator => &[
            Permission::ExecuteTasks,
            Permission::ViewLogs,
            Permission::SendMessages,
            Permission::ViewStatus,
        ],
    }
}

// =============================================================================
// Authorization Check
// =============================================================================

/// Check whether the given role has the required permission.
///
/// Returns `Ok(())` if the role's permission set includes the requested
/// permission, or `Err(SecurityError::InsufficientPermissions)` with the
/// minimum required role if denied.
///
/// # Examples
///
/// ```
/// use common::models::Role;
/// use security_layer::rbac::{authorize, Permission};
///
/// // Admin can manage users
/// assert!(authorize(&Role::Admin, &Permission::ManageUsers).is_ok());
///
/// // Operator cannot manage users
/// assert!(authorize(&Role::Operator, &Permission::ManageUsers).is_err());
///
/// // Operator can execute tasks
/// assert!(authorize(&Role::Operator, &Permission::ExecuteTasks).is_ok());
/// ```
pub fn authorize(role: &Role, permission: &Permission) -> Result<(), SecurityError> {
    let granted = permissions_for_role(role);
    if granted.contains(permission) {
        Ok(())
    } else {
        Err(SecurityError::InsufficientPermissions {
            required_role: permission.required_role(),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_all_permissions() {
        let all_permissions = [
            Permission::ManageUsers,
            Permission::ManageRoles,
            Permission::ManageConfig,
            Permission::ManageSecurity,
            Permission::ExecuteTasks,
            Permission::ViewLogs,
            Permission::SendMessages,
            Permission::ViewStatus,
        ];

        for perm in &all_permissions {
            assert!(
                authorize(&Role::Admin, perm).is_ok(),
                "Admin should have permission {:?}",
                perm
            );
        }
    }

    #[test]
    fn operator_has_operational_permissions() {
        let allowed = [
            Permission::ExecuteTasks,
            Permission::ViewLogs,
            Permission::SendMessages,
            Permission::ViewStatus,
        ];

        for perm in &allowed {
            assert!(
                authorize(&Role::Operator, perm).is_ok(),
                "Operator should have permission {:?}",
                perm
            );
        }
    }

    #[test]
    fn operator_denied_admin_permissions() {
        let denied = [
            Permission::ManageUsers,
            Permission::ManageRoles,
            Permission::ManageConfig,
            Permission::ManageSecurity,
        ];

        for perm in &denied {
            let result = authorize(&Role::Operator, perm);
            assert!(
                result.is_err(),
                "Operator should NOT have permission {:?}",
                perm
            );

            // Verify the error contains the correct required role
            match result.unwrap_err() {
                SecurityError::InsufficientPermissions { required_role } => {
                    assert_eq!(required_role, Role::Admin);
                }
                other => panic!("Expected InsufficientPermissions, got: {:?}", other),
            }
        }
    }

    #[test]
    fn permissions_for_admin_includes_all() {
        let admin_perms = permissions_for_role(&Role::Admin);
        assert_eq!(admin_perms.len(), 8);
    }

    #[test]
    fn permissions_for_operator_excludes_admin_only() {
        let operator_perms = permissions_for_role(&Role::Operator);
        assert_eq!(operator_perms.len(), 4);
        assert!(!operator_perms.contains(&Permission::ManageUsers));
        assert!(!operator_perms.contains(&Permission::ManageRoles));
        assert!(!operator_perms.contains(&Permission::ManageConfig));
        assert!(!operator_perms.contains(&Permission::ManageSecurity));
    }

    #[test]
    fn permission_required_role_is_correct() {
        assert_eq!(Permission::ManageUsers.required_role(), Role::Admin);
        assert_eq!(Permission::ManageRoles.required_role(), Role::Admin);
        assert_eq!(Permission::ManageConfig.required_role(), Role::Admin);
        assert_eq!(Permission::ManageSecurity.required_role(), Role::Admin);
        assert_eq!(Permission::ExecuteTasks.required_role(), Role::Operator);
        assert_eq!(Permission::ViewLogs.required_role(), Role::Operator);
        assert_eq!(Permission::SendMessages.required_role(), Role::Operator);
        assert_eq!(Permission::ViewStatus.required_role(), Role::Operator);
    }
}
