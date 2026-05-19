//! Property-based tests for the RBAC authorization engine.
//!
//! **Validates: Requirements 10.6**
//!
//! - Property 20: RBAC Authorization Correctness

use proptest::prelude::*;

use common::models::Role;
use security_layer::rbac::{authorize, permissions_for_role, Permission};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary Role (Admin or Operator).
fn role_strategy() -> impl Strategy<Value = Role> {
    prop_oneof![Just(Role::Admin), Just(Role::Operator),]
}

/// Generate an arbitrary Permission from the full set.
fn permission_strategy() -> impl Strategy<Value = Permission> {
    prop_oneof![
        Just(Permission::ManageUsers),
        Just(Permission::ManageRoles),
        Just(Permission::ManageConfig),
        Just(Permission::ManageSecurity),
        Just(Permission::ExecuteTasks),
        Just(Permission::ViewLogs),
        Just(Permission::SendMessages),
        Just(Permission::ViewStatus),
    ]
}

// ============================================================================
// Property 20: RBAC Authorization Correctness
//
// For any (role, permission) pair, authorize() SHALL return Ok if and only if
// the permission is in the role's permission set (as returned by
// permissions_for_role).
//
// **Validates: Requirements 10.6**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// **Validates: Requirements 10.6**
    ///
    /// Property 20: For any randomly selected role and permission,
    /// authorize(role, permission) returns Ok(()) if and only if
    /// permissions_for_role(role) contains that permission.
    #[test]
    fn prop_authorize_returns_ok_iff_permission_in_role_set(
        role in role_strategy(),
        permission in permission_strategy()
    ) {
        let granted = permissions_for_role(&role);
        let result = authorize(&role, &permission);

        if granted.contains(&permission) {
            prop_assert!(
                result.is_ok(),
                "authorize({:?}, {:?}) should return Ok because {:?} is in the role's permission set, but got {:?}",
                role, permission, permission, result
            );
        } else {
            prop_assert!(
                result.is_err(),
                "authorize({:?}, {:?}) should return Err because {:?} is NOT in the role's permission set, but got Ok",
                role, permission, permission
            );
        }
    }

    /// **Validates: Requirements 10.6**
    ///
    /// Property 20 (converse): For any role, every permission returned by
    /// permissions_for_role MUST be authorized successfully.
    #[test]
    fn prop_all_granted_permissions_are_authorized(
        role in role_strategy()
    ) {
        let granted = permissions_for_role(&role);
        for permission in granted {
            let result = authorize(&role, permission);
            prop_assert!(
                result.is_ok(),
                "authorize({:?}, {:?}) should succeed for a permission in the role's set, but got {:?}",
                role, permission, result
            );
        }
    }

    /// **Validates: Requirements 10.6**
    ///
    /// Property 20 (denial correctness): For any (role, permission) pair where
    /// authorization is denied, the error SHALL indicate the correct required role.
    #[test]
    fn prop_denied_authorization_reports_correct_required_role(
        role in role_strategy(),
        permission in permission_strategy()
    ) {
        let granted = permissions_for_role(&role);
        let result = authorize(&role, &permission);

        if !granted.contains(&permission) {
            match result {
                Err(common::errors::SecurityError::InsufficientPermissions { required_role }) => {
                    prop_assert_eq!(
                        required_role,
                        permission.required_role(),
                        "Denied authorization for {:?} should report required_role={:?}, got {:?}",
                        permission, permission.required_role(), required_role
                    );
                }
                other => {
                    prop_assert!(
                        false,
                        "Expected InsufficientPermissions error for ({:?}, {:?}), got {:?}",
                        role, permission, other
                    );
                }
            }
        }
    }
}
