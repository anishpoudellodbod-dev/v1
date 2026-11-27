#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Hash, Copy)]
pub enum ResourceName {
    VehicleResource,
    UserResource,
    BillResource,
    MemberResource,
    ReportResource,
    SettingResource,
    PermissionResource,
    CacheResource,
    VehicleCategoryResource,
    BillingRuleResource,
    BillingEntityResource,
    BranchResource,
    ActionLogsResource,
    AuditTrailResource,
    VehicleHistoryResource,
}

impl ResourceName {
    #[inline]
    pub fn from_str_case_insensitive(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("VehicleResource") {
            Some(Self::VehicleResource)
        } else if trimmed.eq_ignore_ascii_case("UserResource") {
            Some(Self::UserResource)
        } else if trimmed.eq_ignore_ascii_case("BillResource") {
            Some(Self::BillResource)
        } else if trimmed.eq_ignore_ascii_case("MemberResource") {
            Some(Self::MemberResource)
        } else if trimmed.eq_ignore_ascii_case("ReportResource") {
            Some(Self::ReportResource)
        } else if trimmed.eq_ignore_ascii_case("SettingResource") {
            Some(Self::SettingResource)
        } else if trimmed.eq_ignore_ascii_case("PermissionResource") {
            Some(Self::PermissionResource)
        } else if trimmed.eq_ignore_ascii_case("CacheResource") {
            Some(Self::CacheResource)
        } else if trimmed.eq_ignore_ascii_case("VehicleCategoryResource") {
            Some(Self::VehicleCategoryResource)
        } else if trimmed.eq_ignore_ascii_case("BillingRuleResource") {
            Some(Self::BillingRuleResource)
        } else if trimmed.eq_ignore_ascii_case("BillingEntityResource") {
            Some(Self::BillingEntityResource)
        } else if trimmed.eq_ignore_ascii_case("BranchResource") {
            Some(Self::BranchResource)
        } else if trimmed.eq_ignore_ascii_case("ActionLogsResource") {
            Some(Self::ActionLogsResource)
        } else if trimmed.eq_ignore_ascii_case("AuditTrailResource") {
            Some(Self::AuditTrailResource)
        } else if trimmed.eq_ignore_ascii_case("VehicleHistoryResource") {
            Some(Self::VehicleHistoryResource)
        } else {
            None
        }
    }
}

impl<'de> serde::Deserialize<'de> for ResourceName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ResourceNameVisitor;

        impl<'de> serde::de::Visitor<'de> for ResourceNameVisitor {
            type Value = ResourceName;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a valid ResourceName string (case-insensitive)")
            }

            fn visit_str<E>(self, value: &str) -> Result<ResourceName, E>
            where
                E: serde::de::Error,
            {
                ResourceName::from_str_case_insensitive(value)
                    .ok_or_else(|| E::custom(format!("Unknown resource name: {}", value)))
            }
        }

        deserializer.deserialize_str(ResourceNameVisitor)
    }
}

impl ToString for ResourceName {
    fn to_string(&self) -> String {
        match self {
            ResourceName::VehicleResource => "vehicleresource".into(),
            ResourceName::UserResource => "UserResource".into(),
            ResourceName::BillResource => "BillResource".into(),
            ResourceName::MemberResource => "MemberResource".into(),
            ResourceName::ReportResource => "ReportResource".into(),
            ResourceName::SettingResource => "SettingResource".into(),
            ResourceName::CacheResource => "CacheResource".into(),
            ResourceName::VehicleCategoryResource => "VehicleCategoryResource".into(),
            ResourceName::PermissionResource => "PermissionResource".into(),
            ResourceName::BillingRuleResource => "BillingRuleResource".into(),
            ResourceName::BillingEntityResource => "BillingEntityResource".into(),
            ResourceName::BranchResource => "BranchResource".into(),
            ResourceName::ActionLogsResource => "ActionLogsResource".into(),
            ResourceName::AuditTrailResource => "AuditTrailResource".into(),
            ResourceName::VehicleHistoryResource => "VehicleHistoryResource".into(),
        }
    }
}

impl From<&str> for ResourceName {
    fn from(s: &str) -> Self {
        Self::from_str_case_insensitive(s).unwrap_or_else(|| panic!("Unknown resource name: {}", s))
    }
}

impl From<std::string::String> for ResourceName {
    fn from(s: std::string::String) -> Self {
        Self::from(s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub struct DatabaseResource {
    pub id: Option<i32>,
    pub resource_name: Option<ResourceName>,
    pub inner_content: Option<serde_json::Value>,
    pub external_content: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_resource_name_to_string_all_variants() {
        assert_eq!(ResourceName::VehicleResource.to_string(), "vehicleresource");
        assert_eq!(ResourceName::UserResource.to_string(), "UserResource");
        assert_eq!(ResourceName::BillResource.to_string(), "BillResource");
        assert_eq!(ResourceName::MemberResource.to_string(), "MemberResource");
        assert_eq!(ResourceName::ReportResource.to_string(), "ReportResource");
        assert_eq!(ResourceName::SettingResource.to_string(), "SettingResource");
        assert_eq!(ResourceName::CacheResource.to_string(), "CacheResource");
        assert_eq!(ResourceName::VehicleCategoryResource.to_string(), "VehicleCategoryResource");
        assert_eq!(ResourceName::PermissionResource.to_string(), "PermissionResource");
        assert_eq!(ResourceName::BillingRuleResource.to_string(), "BillingRuleResource");
        assert_eq!(ResourceName::BillingEntityResource.to_string(), "BillingEntityResource");
        assert_eq!(ResourceName::BranchResource.to_string(), "BranchResource");
        assert_eq!(ResourceName::ActionLogsResource.to_string(), "ActionLogsResource");
        assert_eq!(ResourceName::AuditTrailResource.to_string(), "AuditTrailResource");
        assert_eq!(ResourceName::VehicleHistoryResource.to_string(), "VehicleHistoryResource");
    }

    #[test]
    fn test_resource_name_from_string_all_variants() {
        assert_eq!(ResourceName::from("vehicleresource".to_string()), ResourceName::VehicleResource);
        assert_eq!(ResourceName::from("VehicleResource".to_string()), ResourceName::VehicleResource);
        assert_eq!(ResourceName::from("UserResource".to_string()), ResourceName::UserResource);
        assert_eq!(ResourceName::from("BillResource".to_string()), ResourceName::BillResource);
        assert_eq!(ResourceName::from("MemberResource".to_string()), ResourceName::MemberResource);
        assert_eq!(ResourceName::from("ReportResource".to_string()), ResourceName::ReportResource);
        assert_eq!(ResourceName::from("SettingResource".to_string()), ResourceName::SettingResource);
        assert_eq!(ResourceName::from("CacheResource".to_string()), ResourceName::CacheResource);
        assert_eq!(ResourceName::from("VehicleCategoryResource".to_string()), ResourceName::VehicleCategoryResource);
        assert_eq!(ResourceName::from("PermissionResource".to_string()), ResourceName::PermissionResource);
        assert_eq!(ResourceName::from("BillingRuleResource".to_string()), ResourceName::BillingRuleResource);
        assert_eq!(ResourceName::from("BillingEntityResource".to_string()), ResourceName::BillingEntityResource);
        assert_eq!(ResourceName::from("BranchResource".to_string()), ResourceName::BranchResource);
        assert_eq!(ResourceName::from("ActionLogsResource".to_string()), ResourceName::ActionLogsResource);
        assert_eq!(ResourceName::from("AuditTrailResource".to_string()), ResourceName::AuditTrailResource);
        assert_eq!(ResourceName::from("VehicleHistoryResource".to_string()), ResourceName::VehicleHistoryResource);
    }

    #[test]
    fn test_resource_name_case_insensitive_from_and_serde() {
        let variants = [
            ("VehicleResource", ResourceName::VehicleResource),
            ("UserResource", ResourceName::UserResource),
            ("BillResource", ResourceName::BillResource),
            ("MemberResource", ResourceName::MemberResource),
            ("ReportResource", ResourceName::ReportResource),
            ("SettingResource", ResourceName::SettingResource),
            ("PermissionResource", ResourceName::PermissionResource),
            ("CacheResource", ResourceName::CacheResource),
            ("VehicleCategoryResource", ResourceName::VehicleCategoryResource),
            ("BillingRuleResource", ResourceName::BillingRuleResource),
            ("BillingEntityResource", ResourceName::BillingEntityResource),
            ("BranchResource", ResourceName::BranchResource),
            ("ActionLogsResource", ResourceName::ActionLogsResource),
            ("AuditTrailResource", ResourceName::AuditTrailResource),
            ("VehicleHistoryResource", ResourceName::VehicleHistoryResource),
        ];

        for (pascal, expected) in variants {
            let lower = pascal.to_lowercase();
            let upper = pascal.to_uppercase();

            assert_eq!(ResourceName::from(pascal.to_string()), expected);
            assert_eq!(ResourceName::from(lower.clone()), expected);
            assert_eq!(ResourceName::from(upper.clone()), expected);
            assert_eq!(ResourceName::from(lower.as_str()), expected);
            assert_eq!(ResourceName::from(upper.as_str()), expected);

            let lower_json = format!("\"{}\"", lower);
            let upper_json = format!("\"{}\"", upper);
            assert_eq!(serde_json::from_str::<ResourceName>(&lower_json).unwrap(), expected);
            assert_eq!(serde_json::from_str::<ResourceName>(&upper_json).unwrap(), expected);
        }
    }

    #[test]
    #[should_panic(expected = "Unknown resource name: UnknownResource")]
    fn test_resource_name_from_string_unknown_panics() {
        let _ = ResourceName::from("UnknownResource".to_string());
    }

    #[test]
    #[should_panic(expected = "Unknown resource name: ")]
    fn test_resource_name_from_string_empty_panics() {
        let _ = ResourceName::from("".to_string());
    }

    #[test]
    fn test_resource_name_copy_and_hash() {
        let r1 = ResourceName::VehicleResource;
        let r2 = r1; // Copy
        assert_eq!(r1, r2);

        let mut set = HashSet::new();
        set.insert(ResourceName::VehicleResource);
        set.insert(ResourceName::UserResource);
        assert!(set.contains(&r2));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_database_resource_serde_full() {
        let resource = DatabaseResource {
            id: Some(42),
            resource_name: Some(ResourceName::BillResource),
            inner_content: Some(serde_json::json!({ "total": 500, "note": "paid" })),
            external_content: Some(serde_json::json!({ "verified": true }))
        };

        let serialized = serde_json::to_string(&resource).expect("serialization failed");
        let deserialized: DatabaseResource = serde_json::from_str(&serialized).expect("deserialization failed");
        assert_eq!(resource, deserialized);
    }

    #[test]
    fn test_database_resource_serde_none_fields() {
        let resource = DatabaseResource {
            id: None,
            resource_name: None,
            inner_content: None,
            external_content: None
        };

        let serialized = serde_json::to_string(&resource).expect("serialization failed");
        let deserialized: DatabaseResource = serde_json::from_str(&serialized).expect("deserialization failed");
        assert_eq!(resource, deserialized);
    }

    #[test]
    fn test_database_resource_clone_and_equality() {
        let res1 = DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::UserResource),
            inner_content: None,
            external_content: None,
        };
        let res2 = res1.clone();
        assert_eq!(res1, res2);

        let res3 = DatabaseResource {
            id: Some(2),
            resource_name: Some(ResourceName::UserResource),
            inner_content: None,
            external_content: None,
        };
        assert_ne!(res1, res3);
    }
}
