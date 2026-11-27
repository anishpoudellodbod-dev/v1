pub mod common;
pub mod export;
pub mod fee_head_wise;
pub mod filter_options;
pub mod income;
pub mod mahasang;
pub mod member_transactions;
pub mod rbac;
pub mod vehicle_category;

pub use common::{DateBasis, DateRangeFilter, PaginationMeta};
pub use export::{export_report_bytes, ExportError, ExportFormat, ExportOutput};
pub use rbac::{resolve_caller_branch_context, CallerBranchContext};

#[cfg(test)]
mod tests {
    use super::*;
    use narayani_auth::Claims;
    use narayani_core::{DatabaseResource, ResourceName, Roles};
    use narayani_date::FiscalYearConfig;
    use narayani_settings::AppSettings;
    use narayani_storage::ReportIndex;
    use narayani_vehicle_history::VehicleAliasMap;
    use serde_json::json;

    fn sample_fixture() -> (
        ReportIndex,
        VehicleAliasMap,
        Vec<DatabaseResource>,
        Vec<DatabaseResource>,
        Vec<DatabaseResource>,
        AppSettings,
    ) {
        let branches = vec![
            DatabaseResource {
                id: Some(1),
                resource_name: Some(ResourceName::BranchResource),
                inner_content: Some(json!({ "id": "1", "name": "Main Branch", "BranchCode": "MAIN" })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(3),
                resource_name: Some(ResourceName::BranchResource),
                inner_content: Some(json!({ "id": "3", "name": "चितवन", "BranchCode": "CIT" })),
                external_content: None,
            },
        ];
        let users = vec![DatabaseResource {
            id: Some(7),
            resource_name: Some(ResourceName::UserResource),
            inner_content: Some(json!({
                "id": "7",
                "name": "Chitwan Staff",
                "email": "chitwan@test.com",
                "branchId": "3",
                "branchName": null
            })),
            external_content: None,
        }];
        let rules = vec![
            DatabaseResource {
                id: Some(10),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(json!({
                    "resourceType": "FEE_HEAD",
                    "feeHeadId": "116",
                    "feeHead": "सदस्यता शुल्क",
                    "fiscalYear": "2082/2083",
                    "displayOrder": 1
                })),
                external_content: None,
            },
            DatabaseResource {
                id: Some(11),
                resource_name: Some(ResourceName::BillingRuleResource),
                inner_content: Some(json!({
                    "resourceType": "FEE_HEAD",
                    "feeHeadId": "99",
                    "feeHead": "Unused Head",
                    "fiscalYear": "2082/2083",
                    "displayOrder": 2
                })),
                external_content: None,
            },
        ];
        let cats = vec![DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::VehicleCategoryResource),
            inner_content: Some(json!({ "category": "ट्रक", "vehicle_types": ["10 pangree"] })),
            external_content: None,
        }];
        let vehicles = vec![DatabaseResource {
            id: Some(50),
            resource_name: Some(ResourceName::VehicleResource),
            inner_content: Some(json!({
                "VehicleId": "V-CURR",
                "VehicleNumber": "Province-3-01-23-KHA-5555",
                "VehicleType": "10 pangree",
                "previousVehicleIds": ["V-OLD"],
                "previousVehicleNumbers": ["Na-1-KA-1111"],
                "Owners": [{ "Name": "Ram", "Phone": "9800000001" }]
            })),
            external_content: None,
        }];
        let members = vec![DatabaseResource {
            id: Some(1),
            resource_name: Some(ResourceName::MemberResource),
            inner_content: Some(json!({
                "vehicleId": "V-CURR",
                "vehicleNumber": "Province-3-01-23-KHA-5555",
                "memberNumber": "MEM-001"
            })),
            external_content: None,
        }];
        let bills = vec![
            // Bill 1: Live-DB null status & Lodbod null billingType, Chitwan branch, previous vehicle alias!
            DatabaseResource {
                id: Some(201),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "paidAt": "2026-11-10T10:00:00Z",
                    "billingDate": "2026-11-10",
                    "periodStart": "2026-11-10",
                    "periodEnd": "2026-12-10",
                    "status": null,
                    "billingType": null,
                    "branch": { "id": "3", "name": "चितवन", "BranchCode": "CIT" },
                    "vehicle": { "id": "V-OLD", "vehicleNumber": "Na-1-KA-1111", "vehicleType": "10 pangree" },
                    "customerName": "=cmd|evil",
                    "fiscalYear": "2082/2083",
                    "total": 2500,
                    "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 2000 }]
                })),
                external_content: None,
            },
            // Bill 2: Main Branch GENERAL bill
            DatabaseResource {
                id: Some(202),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "paidAt": "2026-11-12T10:00:00Z",
                    "billingDate": "2026-11-12",
                    "periodStart": "2026-11-12",
                    "periodEnd": "2026-12-12",
                    "status": "PAID",
                    "billingType": "GENERAL",
                    "branch": { "id": "1", "name": "Main Branch", "BranchCode": "MAIN" },
                    "vehicle": { "id": "V-OTHER", "vehicleNumber": "BA-01-KHA-9999", "vehicleType": "" },
                    "fiscalYear": "2082/2083",
                    "total": 500,
                    "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 500 }]
                })),
                external_content: None,
            },
            // Bill 3: CANCELLED bill (should contribute 0 to monetary sums)
            DatabaseResource {
                id: Some(203),
                resource_name: Some(ResourceName::BillResource),
                inner_content: Some(json!({
                    "paidAt": "2026-11-15T10:00:00Z",
                    "billingDate": "2026-11-15",
                    "periodStart": "2026-11-15",
                    "periodEnd": "2026-12-15",
                    "status": "CANCELLED",
                    "billingType": "MEMBERSHIP",
                    "branch": { "id": "3", "name": "चितवन", "BranchCode": "CIT" },
                    "vehicle": { "id": "V-CURR", "vehicleNumber": "Province-3-01-23-KHA-5555", "vehicleType": "10 pangree" },
                    "fiscalYear": "2082/2083",
                    "total": 9999,
                    "items": [{ "feeHeadId": "116", "title": "सदस्यता शुल्क", "amount": 9999 }]
                })),
                external_content: None,
            },
        ];

        let mut fy = FiscalYearConfig::from_settings(&[]);
        let mut idx = ReportIndex::new();
        for b in &bills {
            idx.upsert_bill(b, &mut fy);
        }
        idx.rebuild_dimensions(
            branches.iter(),
            users.iter(),
            rules.iter(),
            cats.iter(),
            &fy,
        );
        let aliases = VehicleAliasMap::build(vehicles.iter(), std::iter::empty());
        let settings = AppSettings::default();
        (idx, aliases, users, vehicles, members, settings)
    }

    #[test]
    fn rbac_locks_non_admin_using_branch_id_join_and_handles_invalid_dates() {
        let (idx, aliases, users, vehicles, _members, settings) = sample_fixture();
        let staff_claims = Claims {
            sub: "chitwan@test.com".into(),
            exp: 9999999999,
            iat: 1000,
            role: Roles::Operator,
            user_id: Some(7),
            jti: None,
            role_name: Some("Staff".into()),
            tenant_id: None,
        };
        let caller = resolve_caller_branch_context(Some(&staff_claims), &users, &idx);
        assert!(!caller.is_admin);
        assert_eq!(caller.assigned_branch_name.as_deref(), Some("चितवन"));

        let fo = filter_options::compute_filter_options(
            &filter_options::FilterOptionsQuery {
                fiscal_year: Some("2082/83".into()),
                branch: Some("Main Branch".into()), // ignored by RBAC
                report_type: Some("mahasang".into()),
            },
            &caller,
            &idx,
            &users,
            &vehicles.iter().collect::<Vec<_>>(),
            &settings,
        );
        assert_eq!(fo.branches, vec!["चितवन".to_string()]);
        assert_eq!(fo.users, vec!["Chitwan Staff".to_string()]);

        // Income sees only Chitwan's non-cancelled bill (201) despite asking for Main Branch
        let inc = income::compute_income_report(
            &income::IncomeReportQuery {
                branch: Some("Main Branch".into()),
                ..Default::default()
            },
            &caller,
            &idx,
            &users,
        );
        assert_eq!(inc.summary.total_transactions, 1);
        assert_eq!(inc.summary.total_income, 2500.0);

        // Inverted date range returns 200-ready empty response
        let bad_date = income::compute_income_report(
            &income::IncomeReportQuery {
                date_from: Some("2026-12-01".into()),
                date_to: Some("2026-11-01".into()),
                ..Default::default()
            },
            &caller,
            &idx,
            &users,
        );
        assert_eq!(bad_date.pagination.total, 0);
        assert!(bad_date.rows.is_empty());
        assert_eq!(bad_date.summary.total_income, 0.0);

        // Fee head wise prunes unused "99" head
        let admin_caller = CallerBranchContext {
            is_admin: true,
            ..Default::default()
        };
        let fhw = fee_head_wise::compute_fee_head_wise_report(
            &fee_head_wise::FeeHeadWiseQuery {
                fiscal_year: Some("2082/83".into()),
                ..Default::default()
            },
            &admin_caller,
            &idx,
            &users,
        );
        assert_eq!(fhw.visible_fee_heads.len(), 1);
        assert_eq!(fhw.visible_fee_heads[0].id, "116");
        assert_eq!(fhw.all_totals.total, 3000.0);

        // Vehicle category resolves "10 pangree" -> "ट्रक" and "" -> "वर्ग उपलब्ध छैन"
        let vc = vehicle_category::compute_vehicle_category_report(
            &vehicle_category::VehicleCategoryQuery::default(),
            &admin_caller,
            &idx,
            &aliases,
        );
        assert_eq!(vc.rows.len(), 2);
        assert!(vc.rows.iter().any(|r| r.category == "ट्रक"));
        assert!(vc.rows.iter().any(|r| r.category == "वर्ग उपलब्ध छैन"));
    }

    #[test]
    fn mahasang_interval_aliases_member_extra_and_exports_work() {
        let (idx, aliases, users, vehicles, members, settings) = sample_fixture();
        let admin = CallerBranchContext {
            is_admin: true,
            ..Default::default()
        };

        // Mahasang interval overlap: 2026-11-20..2026-11-25 overlaps periodEnd 2026-12-10
        let m_resp = mahasang::compute_mahasang_report(
            &mahasang::MahasangReportRequest {
                date_basis: Some("billingDate".into()),
                date_from: Some("2026-11-20".into()),
                date_to: Some("2026-11-25".into()),
                vehicle_types: Some(vec![]),
                ..Default::default()
            },
            &admin,
            &idx,
            &aliases,
            &settings,
        );
        // Includes PAID 201, PAID 202, and CANCELLED 203 (with 0 money)
        assert_eq!(m_resp.summary.total_bills, 3);
        assert_eq!(m_resp.summary.total_membership, 2500.0);
        assert_eq!(m_resp.summary.total_other, 500.0);
        let row201 = m_resp.rows.iter().find(|r| r.id == "201").unwrap();
        assert_eq!(row201.vehicle_number, "Province-3-01-23-KHA-5555");
        assert_eq!(row201.previous_vehicle_number, "Na-1-KA-1111");

        // Member transactions extraAmount calculation (2500 total - 2000 items = 500 extra)
        let mt = member_transactions::compute_member_transactions(
            "Province-3-01-23-KHA-5555",
            &member_transactions::MemberTransactionsQuery::default(),
            &admin,
            &idx,
            &aliases,
            &members.iter().collect::<Vec<_>>(),
            &vehicles.iter().collect::<Vec<_>>(),
        );
        assert_eq!(mt.profile.member_number, "MEM-001");
        assert_eq!(mt.total_paid, 2500.0);
        assert_eq!(mt.total_extra, 500.0);

        // Exports: CSV formula sanitization + XLSX bytes + PDF 501
        let (exp_rows, m, o, g) = income::build_amdani_export_rows(
            &income::IncomeReportQuery::default(),
            &admin,
            &idx,
            &users,
        );
        let csv_out = export::export_income(ExportFormat::Csv, &exp_rows, m, o, g).unwrap();
        let csv_str = String::from_utf8(csv_out.bytes).unwrap();
        assert!(csv_str.contains("'=cmd|evil"));

        let xlsx_out = export::export_income(ExportFormat::Xlsx, &exp_rows, m, o, g).unwrap();
        assert!(xlsx_out.bytes.starts_with(b"PK"));

        let pdf_res = export::export_income(ExportFormat::Pdf, &exp_rows, m, o, g);
        assert!(matches!(pdf_res, Err(ExportError::PdfNotImplemented)));
    }
}
