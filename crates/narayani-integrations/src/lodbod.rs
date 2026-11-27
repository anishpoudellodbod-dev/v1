use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

pub fn shared_http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LodbodPaymentUpdatePayload {
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "occurredAt")]
    pub occurred_at: String,
    #[serde(rename = "organizationCode")]
    pub organization_code: String,
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "vehicleNumber")]
    pub vehicle_number: String,
    #[serde(rename = "billingId")]
    pub billing_id: String,
    #[serde(rename = "feeId")]
    pub fee_id: String,
    #[serde(rename = "feeTitle")]
    pub fee_title: String,
    #[serde(rename = "fiscalYear")]
    pub fiscal_year: String,
    #[serde(rename = "billingPeriod")]
    pub billing_period: String,
    pub amount: String,
    pub currency: String,
    #[serde(rename = "paymentStatus")]
    pub payment_status: String,
    #[serde(rename = "paymentMethod")]
    pub payment_method: String,
    #[serde(rename = "paymentDate")]
    pub payment_date: String,
    #[serde(rename = "receiptNumber")]
    pub receipt_number: String,
    #[serde(rename = "dueDate", skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(rename = "lodbodPaymentReferenceId")]
    pub lodbod_payment_reference_id: Option<String>,
    #[serde(rename = "esewaTransactionId")]
    pub esewa_transaction_id: Option<String>,
}

pub async fn send_payment_update_to_lodbod(
    payload: &LodbodPaymentUpdatePayload,
    receiver_url: &str,
) -> Result<(), reqwest::Error> {
    shared_http_client()
        .post(receiver_url)
        .json(payload)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}
