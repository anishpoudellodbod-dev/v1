#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash, Copy)]
pub enum ResourceName{
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
        }
    }
}

impl From<std::string::String> for ResourceName {
    fn from(s: std::string::String) -> Self {
        match s.as_str() {
            "vehicleresource" => ResourceName::VehicleResource,
            "VehicleResource" => ResourceName::VehicleResource,
            "UserResource" => ResourceName::UserResource,
            "BillResource" => ResourceName::BillResource,
            "MemberResource" => ResourceName::MemberResource,
            "ReportResource" => ResourceName::ReportResource,
            "SettingResource" => ResourceName::SettingResource,
            "CacheResource" => ResourceName::CacheResource,
            "VehicleCategoryResource" => ResourceName::VehicleCategoryResource,
            "PermissionResource" => ResourceName::PermissionResource,
            "BillingRuleResource" => ResourceName::BillingRuleResource,
            "BillingEntityResource" => ResourceName::BillingEntityResource,
            "BranchResource" => ResourceName::BranchResource,
            _ => panic!("Unknown resource name: {}", s),
        }
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
