use super::common::sanitize_csv_cell;
use super::fee_head_wise::FeeHeadWiseResponse;
use super::income::AmdaniExportRow;
use super::mahasang::{MahasangRow, MahasangSummary};
use super::member_transactions::MemberTransactionsResponse;
use super::vehicle_category::{VehicleCategoryRow, VehicleCategoryTotals};
use rust_xlsxwriter::{Format, Workbook};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Xlsx,
    Csv,
    Pdf,
}

#[derive(Debug, Clone)]
pub enum ExportError {
    UnsupportedFormat(String),
    PdfNotImplemented,
    GenerationFailed(String),
}

#[derive(Debug, Clone)]
pub struct ExportOutput {
    pub content_type: &'static str,
    pub filename: String,
    pub bytes: Vec<u8>,
}

pub fn parse_export_format(raw: Option<&str>, allow_csv: bool) -> Result<ExportFormat, ExportError> {
    let f = raw.unwrap_or("xlsx").trim().to_ascii_lowercase();
    match f.as_str() {
        "" | "xlsx" => Ok(ExportFormat::Xlsx),
        "csv" if allow_csv => Ok(ExportFormat::Csv),
        "pdf" => Ok(ExportFormat::Pdf),
        other => Err(ExportError::UnsupportedFormat(format!(
            "Unsupported export format '{other}'"
        ))),
    }
}

fn write_csv_bytes(headers: &[&str], rows: &[Vec<String>]) -> Result<Vec<u8>, ExportError> {
    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record(headers)
        .map_err(|e| ExportError::GenerationFailed(e.to_string()))?;
    for row in rows {
        let sanitized: Vec<String> = row.iter().map(|c| sanitize_csv_cell(c)).collect();
        wtr.write_record(&sanitized)
            .map_err(|e| ExportError::GenerationFailed(e.to_string()))?;
    }
    wtr.into_inner()
        .map_err(|e| ExportError::GenerationFailed(e.to_string()))
}

fn build_xlsx_table(
    sheet_name: &str,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<Vec<u8>, ExportError> {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let _ = ws.set_name(sheet_name);
    let bold = Format::new().set_bold();

    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &bold)
            .map_err(|e| ExportError::GenerationFailed(e.to_string()))?;
    }
    for (r_idx, row) in rows.iter().enumerate() {
        let excel_row = (r_idx + 1) as u32;
        for (c_idx, val) in row.iter().enumerate() {
            ws.write_string(excel_row, c_idx as u16, val)
                .map_err(|e| ExportError::GenerationFailed(e.to_string()))?;
        }
    }
    wb.save_to_buffer()
        .map_err(|e| ExportError::GenerationFailed(e.to_string()))
}

pub fn export_report_bytes(
    fmt: ExportFormat,
    base_name: &str,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<ExportOutput, ExportError> {
    match fmt {
        ExportFormat::Pdf => Err(ExportError::PdfNotImplemented),
        ExportFormat::Csv => {
            let bytes = write_csv_bytes(headers, rows)?;
            Ok(ExportOutput {
                content_type: "text/csv; charset=utf-8",
                filename: format!("{base_name}.csv"),
                bytes,
            })
        }
        ExportFormat::Xlsx => {
            let bytes = build_xlsx_table(base_name, headers, rows)?;
            Ok(ExportOutput {
                content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                filename: format!("{base_name}.xlsx"),
                bytes,
            })
        }
    }
}

pub fn export_income(
    fmt: ExportFormat,
    rows: &[AmdaniExportRow],
    total_mem: f64,
    total_other: f64,
    total_grand: f64,
) -> Result<ExportOutput, ExportError> {
    let headers = [
        "क्रम संख्या",
        "मिति",
        "रसिद नं.",
        "सवारी नं.",
        "प्रकार",
        "धनी / संचालकको नाम",
        "सदस्यता शुल्क",
        "अन्य शुल्क",
        "जम्मा रकम",
        "भुक्तानी माध्यम",
        "शाखा",
        "प्रयोगकर्ता",
        "आर्थिक वर्ष",
        "कैफियत",
    ];
    let mut table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.sn.to_string(),
                r.date.clone(),
                r.receipt_number.clone(),
                r.vehicle_number.clone(),
                r.vehicle_type.clone(),
                r.owner_operator_display.clone(),
                format!("{:.2}", r.membership_amount),
                format!("{:.2}", r.other_amount),
                format!("{:.2}", r.receipt_total),
                r.payment_method.clone(),
                r.branch_name.clone(),
                r.user_name.clone(),
                r.fiscal_year.clone(),
                r.note.clone(),
            ]
        })
        .collect();
    // Summary footer
    table_rows.push(vec![
        String::new(),
        String::new(),
        format!("कुल संख्या: {}", rows.len()),
        String::new(),
        String::new(),
        "जम्मा".to_string(),
        format!("सदस्यता: {:.2}", total_mem),
        format!("अन्य: {:.2}", total_other),
        format!("जम्मा रू. {:.2}", total_grand),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    export_report_bytes(fmt, "income-report", &headers, &table_rows)
}

pub fn export_fee_head_wise(
    fmt: ExportFormat,
    data: &FeeHeadWiseResponse,
) -> Result<ExportOutput, ExportError> {
    let mut headers: Vec<String> = vec![
        "क्रम संख्या".to_string(),
        "मिति".to_string(),
        "रसिद नं.".to_string(),
        "सवारी नं.".to_string(),
        "धनीको नाम".to_string(),
    ];
    for fh in &data.visible_fee_heads {
        headers.push(fh.name.clone());
    }
    headers.push("जम्मा रकम".to_string());

    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    let mut table_rows: Vec<Vec<String>> = data
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = vec![
                (i + 1).to_string(),
                r.billing_date.clone(),
                r.receipt_number.clone(),
                r.vehicle_number.clone(),
                r.customer_name.clone(),
            ];
            for fh in &data.visible_fee_heads {
                let amt = r.amounts.get(&fh.id).copied().unwrap_or(0.0);
                row.push(format!("{amt:.2}"));
            }
            row.push(format!("{:.2}", r.total_amount));
            row
        })
        .collect();

    let mut footer = vec![
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        "जम्मा".to_string(),
    ];
    for fh in &data.visible_fee_heads {
        let amt = data.all_totals.amounts.get(&fh.id).copied().unwrap_or(0.0);
        footer.push(format!("{amt:.2}"));
    }
    footer.push(format!("{:.2}", data.all_totals.total));
    table_rows.push(footer);

    export_report_bytes(fmt, "fee-head-wise-report", &header_refs, &table_rows)
}

pub fn export_vehicle_category(
    fmt: ExportFormat,
    rows: &[VehicleCategoryRow],
    totals: &VehicleCategoryTotals,
) -> Result<ExportOutput, ExportError> {
    let headers = [
        "क्रम संख्या",
        "रसिद नं.",
        "सवारी नं.",
        "वर्ग",
        "सवारी प्रकार",
        "रुट",
        "रुट पर्मिट म्याद",
        "धनीको नाम",
        "सदस्यता शुल्क",
        "अन्य शुल्क",
        "जम्मा",
        "बिल मिति",
    ];
    let mut table_rows: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                (i + 1).to_string(),
                r.receipt_number.clone(),
                r.vehicle_number.clone(),
                r.category.clone(),
                r.vehicle_type.clone(),
                r.route.clone(),
                r.route_permit_expiry.clone(),
                r.owner_name.clone(),
                format!("{:.2}", r.membership_income),
                format!("{:.2}", r.other_income),
                format!("{:.2}", r.total),
                r.billing_date.clone(),
            ]
        })
        .collect();
    table_rows.push(vec![
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        "जम्मा".to_string(),
        format!("{:.2}", totals.membership),
        format!("{:.2}", totals.other),
        format!("{:.2}", totals.total),
        String::new(),
    ]);
    export_report_bytes(fmt, "vehicle-category-report", &headers, &table_rows)
}

pub fn export_mahasang(
    fmt: ExportFormat,
    rows: &[MahasangRow],
    summary: &MahasangSummary,
    visible_columns: Option<&[String]>,
) -> Result<ExportOutput, ExportError> {
    let all_cols: &[(&str, &str)] = &[
        ("vehicleNumber", "सवारी नं."),
        ("previousVehicleNumber", "पुरानो सवारी नं."),
        ("vehicleType", "प्रकार"),
        ("ownerName", "धनीको नाम"),
        ("operatorName", "संचालकको नाम"),
        ("ownerPhone", "फोन नं."),
        ("receiptNumber", "रसिद नं."),
        ("membershipIncome", "सदस्यता"),
        ("otherIncome", "अन्य"),
        ("total", "जम्मा"),
        ("billingDate", "बिल मिति"),
        ("periodStart", "सुरु मिति"),
        ("periodEnd", "समाप्ति मिति"),
        ("note", "कैफियत"),
    ];

    let active_cols: Vec<(&str, &str)> = match visible_columns {
        Some(vis) if !vis.is_empty() => all_cols
            .iter()
            .copied()
            .filter(|(key, _)| vis.iter().any(|v| v.eq_ignore_ascii_case(key)))
            .collect(),
        _ => all_cols.to_vec(),
    };
    let active_cols = if active_cols.is_empty() {
        all_cols.to_vec()
    } else {
        active_cols
    };

    let headers: Vec<&str> = active_cols.iter().map(|(_, label)| *label).collect();
    let mut table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            active_cols
                .iter()
                .map(|(k, _)| match *k {
                    "vehicleNumber" => r.vehicle_number.clone(),
                    "previousVehicleNumber" => r.previous_vehicle_number.clone(),
                    "vehicleType" => r.vehicle_type.clone(),
                    "ownerName" => r.owner_name.clone(),
                    "operatorName" => r.operator_name.clone(),
                    "ownerPhone" => r.owner_phone.clone(),
                    "receiptNumber" => r.receipt_number.clone(),
                    "membershipIncome" => format!("{:.2}", r.membership_income),
                    "otherIncome" => format!("{:.2}", r.other_income),
                    "total" => format!("{:.2}", r.total),
                    "billingDate" => r.billing_date.clone(),
                    "periodStart" => r.period_start.clone(),
                    "periodEnd" => r.period_end.clone(),
                    "note" => r.note.clone(),
                    _ => String::new(),
                })
                .collect()
        })
        .collect();

    let footer: Vec<String> = active_cols
        .iter()
        .map(|(k, _)| match *k {
            "receiptNumber" => format!("कुल रसिद: {}", summary.total_bills),
            "membershipIncome" => format!("{:.2}", summary.total_membership),
            "otherIncome" => format!("{:.2}", summary.total_other),
            "total" => format!("{:.2}", summary.total_membership + summary.total_other),
            _ => String::new(),
        })
        .collect();
    table_rows.push(footer);

    export_report_bytes(fmt, "mahasang-report", &headers, &table_rows)
}

pub fn export_member_transactions(
    fmt: ExportFormat,
    data: &MemberTransactionsResponse,
) -> Result<ExportOutput, ExportError> {
    let headers = [
        "क्रम संख्या",
        "रसिद नं.",
        "भुक्तानी मिति",
        "सुरु मिति",
        "अन्तिम मिति",
        "महिना",
        "भुक्तानी माध्यम",
        "शाखा",
        "अतिरिक्त रकम",
        "जम्मा रकम",
        "स्थिति",
    ];
    let mut table_rows: Vec<Vec<String>> = data
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                (i + 1).to_string(),
                r.receipt_number.clone(),
                r.payment_date.clone(),
                r.period_start.clone(),
                r.period_end.clone(),
                r.billing_months.to_string(),
                r.payment_method.clone(),
                r.branch_name.clone(),
                format!("{:.2}", r.extra_amount),
                format!("{:.2}", r.total_amount),
                r.status.clone(),
            ]
        })
        .collect();
    table_rows.push(vec![
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        "जम्मा".to_string(),
        format!("{:.2}", data.total_extra),
        format!("{:.2}", data.total_paid),
        String::new(),
    ]);
    export_report_bytes(fmt, "member-transactions", &headers, &table_rows)
}
