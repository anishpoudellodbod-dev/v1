/*
The number Province-4-15-23-KHA-5555 appears to follow Nepal's provincial vehicle registration numbering system.

Here is a breakdown of its components:

Component

Meaning

Province-4

Province No. 4, now known as Gandaki Province

15

Transport Management Office or registration office code

23

Vehicle group/lot or registration series indicator

KHA (ख)

Vehicle category code

5555

Individual vehicle registration number

Meaning of KHA (ख)
In Nepal's traditional vehicle registration classification:

क (KA) – Private heavy vehicle

ख (KHA) – Public/commercial heavy vehicle

ग (GA) – Government heavy vehicle

Therefore, KHA generally indicates a public or commercial heavy vehicle, such as a bus or truck.

Amarnepal
+1

Overall interpretation
Province-4-15-23-KHA-5555 can be interpreted as a vehicle registered in Gandaki Province, under transport office code 15, in group 23, with registration number 5555.

Important: The exact meanings of 15 and 23 should be verified against the relevant Transport Management Office's registration records. Also, this is not the same as the newer embossed number plate format
 
 */

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleParseError {
    EmptyInput,
    InvalidFormat(String),
    InvalidStateOrZone(String),
    InvalidTransportOffice(String),
    InvalidGroupCode(String),
    InvalidCategory(String),
    InvalidRegistrationNumber(String),
}

impl fmt::Display for VehicleParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "Vehicle number string cannot be empty"),
            Self::InvalidFormat(s) => write!(f, "Invalid vehicle number format: '{s}'"),
            Self::InvalidStateOrZone(s) => write!(f, "Unrecognized province, state, or zone: '{s}'"),
            Self::InvalidTransportOffice(s) => write!(f, "Invalid transport office code: '{s}'"),
            Self::InvalidGroupCode(s) => write!(f, "Invalid group/lot code: '{s}'"),
            Self::InvalidCategory(s) => write!(f, "Invalid vehicle category code: '{s}'"),
            Self::InvalidRegistrationNumber(s) => write!(f, "Invalid registration number: '{s}'"),
        }
    }
}

impl std::error::Error for VehicleParseError {}

// Supports all 7 Provinces of Nepal (P1..P7) and all 14 Zones of Nepal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VehicleState {
    // 7 Provinces (State / Provincial format)
    P1, // Koshi Province (Province 1)
    P2, // Madhesh Province (Province 2)
    P3, // Bagmati Province (Province 3)
    P4, // Gandaki Province (Province 4)
    P5, // Lumbini Province (Province 5)
    P6, // Karnali Province (Province 6)
    P7, // Sudurpashchim Province (Province 7)

    // 14 Zones (Zonal format)
    Mechi,       // ME
    Koshi,       // KO
    Sagarmatha,  // SA
    Janakpur,    // JA
    Bagmati,     // BA
    Narayani,    // NA
    Gandaki,     // GA
    Lumbini,     // LU
    Dhaulagiri,  // DHA / DHAU
    Rapti,       // RA
    Karnali,     // KA
    Bheri,       // BHE
    Seti,        // SE
    Mahakali,    // MA
}

impl VehicleState {
    pub fn is_provincial(&self) -> bool {
        matches!(
            self,
            Self::P1 | Self::P2 | Self::P3 | Self::P4 | Self::P5 | Self::P6 | Self::P7
        )
    }

    pub fn is_zonal(&self) -> bool {
        !self.is_provincial()
    }

    /// Parses a province/state identifier (used when the plate has 5 components or explicit province markers).
    pub fn parse_provincial(s: &str) -> Result<Self, VehicleParseError> {
        let normalized = normalize_token(s)
            .ok_or_else(|| VehicleParseError::InvalidStateOrZone(s.to_string()))?;
        match normalized.as_str() {
            // Province 1 / Koshi
            "P1" | "PROVINCE1" | "PRADESH1" | "STATE1" | "PROVINCE01" | "PRADESH01" | "STATE01"
            | "KOPRA" | "KOSHIPRA" | "KOSHIPRADESH" | "KOSHIPROVINCE" | "KOSHISTATE" | "KOSHI" => {
                Ok(Self::P1)
            }

            // Province 2 / Madhesh
            "P2" | "PROVINCE2" | "PRADESH2" | "STATE2" | "PROVINCE02" | "PRADESH02" | "STATE02"
            | "MAPRA" | "MADHESHPRA" | "MADHESHPRADESH" | "MADHESHPROVINCE" | "MADHESHSTATE"
            | "MADHESH" | "MADESHPRA" | "MADESHPRADESH" | "MADESHPROVINCE" | "MADESH" => {
                Ok(Self::P2)
            }

            // Province 3 / Bagmati
            "P3" | "PROVINCE3" | "PRADESH3" | "STATE3" | "PROVINCE03" | "PRADESH03" | "STATE03"
            | "BAPRA" | "BAGMATIPRA" | "BAGMATIPRADESH" | "BAGMATIPROVINCE" | "BAGMATISTATE"
            | "BAGMATI" => Ok(Self::P3),

            // Province 4 / Gandaki
            "P4" | "PROVINCE4" | "PRADESH4" | "STATE4" | "PROVINCE04" | "PRADESH04" | "STATE04"
            | "GAPRA" | "GANDAKIPRA" | "GANDAKIPRADESH" | "GANDAKIPROVINCE" | "GANDAKISTATE"
            | "GANDAKI" => Ok(Self::P4),

            // Province 5 / Lumbini
            "P5" | "PROVINCE5" | "PRADESH5" | "STATE5" | "PROVINCE05" | "PRADESH05" | "STATE05"
            | "LUPRA" | "LUMBINIPRA" | "LUMBINIPRADESH" | "LUMBINIPROVINCE" | "LUMBINISTATE"
            | "LUMBINI" => Ok(Self::P5),

            // Province 6 / Karnali
            "P6" | "PROVINCE6" | "PRADESH6" | "STATE6" | "PROVINCE06" | "PRADESH06" | "STATE06"
            | "KAPRA" | "KARNALIPRA" | "KARNALIPRADESH" | "KARNALIPROVINCE" | "KARNALISTATE"
            | "KARNALI" => Ok(Self::P6),

            // Province 7 / Sudurpashchim
            "P7" | "PROVINCE7" | "PRADESH7" | "STATE7" | "PROVINCE07" | "PRADESH07" | "STATE07"
            | "SUPAPRA" | "SUPRA" | "SUDURPASHCHIMPRA" | "SUDURPASHCHIMPRADESH"
            | "SUDURPASHCHIMPROVINCE" | "SUDURPASHCHIMSTATE" | "SUDURPASHCHIM"
            | "SUDURPASCHIMPRA" | "SUDURPASCHIMPRADESH" | "SUDURPASCHIMPROVINCE"
            | "SUDURPASCHIM" => Ok(Self::P7),

            _ => Err(VehicleParseError::InvalidStateOrZone(s.to_string())),
        }
    }

    /// Parses a zonal identifier (used when the plate has 4 components).
    pub fn parse_zonal(s: &str) -> Result<Self, VehicleParseError> {
        let normalized = normalize_token(s)
            .ok_or_else(|| VehicleParseError::InvalidStateOrZone(s.to_string()))?;
        match normalized.as_str() {
            "ME" | "MECHI" => Ok(Self::Mechi),
            "KO" | "KOSHI" => Ok(Self::Koshi),
            "SA" | "SAGARMATHA" => Ok(Self::Sagarmatha),
            "JA" | "JANAKPUR" => Ok(Self::Janakpur),
            "BA" | "BAGMATI" => Ok(Self::Bagmati),
            "NA" | "NARAYANI" => Ok(Self::Narayani),
            "GA" | "GANDAKI" => Ok(Self::Gandaki),
            "LU" | "LUMBINI" => Ok(Self::Lumbini),
            "DHA" | "DHAU" | "DHAULAGIRI" => Ok(Self::Dhaulagiri),
            "RA" | "RAPTI" => Ok(Self::Rapti),
            "KA" | "KARNALI" => Ok(Self::Karnali),
            "BHE" | "BHERI" => Ok(Self::Bheri),
            "SE" | "SETI" => Ok(Self::Seti),
            "MA" | "MAHAKALI" => Ok(Self::Mahakali),
            _ => Err(VehicleParseError::InvalidStateOrZone(s.to_string())),
        }
    }
}

impl FromStr for VehicleState {
    type Err = VehicleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = normalize_token(s)
            .ok_or_else(|| VehicleParseError::InvalidStateOrZone(s.to_string()))?;
        match normalized.as_str() {
            "ME" | "MECHI" => Ok(Self::Mechi),
            "KO" => Ok(Self::Koshi),
            "SA" | "SAGARMATHA" => Ok(Self::Sagarmatha),
            "JA" | "JANAKPUR" => Ok(Self::Janakpur),
            "BA" => Ok(Self::Bagmati),
            "NA" | "NARAYANI" => Ok(Self::Narayani),
            "GA" => Ok(Self::Gandaki),
            "LU" => Ok(Self::Lumbini),
            "DHA" | "DHAU" | "DHAULAGIRI" => Ok(Self::Dhaulagiri),
            "RA" | "RAPTI" => Ok(Self::Rapti),
            "KA" => Ok(Self::Karnali),
            "BHE" | "BHERI" => Ok(Self::Bheri),
            "SE" | "SETI" => Ok(Self::Seti),
            "MA" | "MAHAKALI" => Ok(Self::Mahakali),
            _ => Self::parse_provincial(s).or_else(|_| Self::parse_zonal(s)),
        }
    }
}

impl fmt::Display for VehicleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let canonical = match self {
            Self::P1 => "Province-1",
            Self::P2 => "Province-2",
            Self::P3 => "Province-3",
            Self::P4 => "Province-4",
            Self::P5 => "Province-5",
            Self::P6 => "Province-6",
            Self::P7 => "Province-7",
            Self::Mechi => "ME",
            Self::Koshi => "KO",
            Self::Sagarmatha => "SA",
            Self::Janakpur => "JA",
            Self::Bagmati => "BA",
            Self::Narayani => "NA",
            Self::Gandaki => "GA",
            Self::Lumbini => "LU",
            Self::Dhaulagiri => "DHA",
            Self::Rapti => "RA",
            Self::Karnali => "KA",
            Self::Bheri => "BHE",
            Self::Seti => "SE",
            Self::Mahakali => "MA",
        };
        write!(f, "{canonical}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportOffice(pub u16);

impl FromStr for TransportOffice {
    type Err = VehicleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let code = trimmed
            .parse::<u16>()
            .map_err(|_| VehicleParseError::InvalidTransportOffice(s.to_string()))?;
        if code == 0 || code > 999 {
            return Err(VehicleParseError::InvalidTransportOffice(s.to_string()));
        }
        Ok(TransportOffice(code))
    }
}

impl fmt::Display for TransportOffice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupCode(pub u16);

impl FromStr for GroupCode {
    type Err = VehicleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let code = trimmed
            .parse::<u16>()
            .map_err(|_| VehicleParseError::InvalidGroupCode(s.to_string()))?;
        if code == 0 || code > 999 {
            return Err(VehicleParseError::InvalidGroupCode(s.to_string()));
        }
        Ok(GroupCode(code))
    }
}

impl fmt::Display for GroupCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Standard Nepal vehicle registration category codes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VehicleCategory {
    KA,   // क - Private heavy vehicle
    KHA,  // ख - Public/commercial heavy vehicle
    GA,   // ग - Government heavy vehicle
    GHA,  // घ - Tourist/diplomatic/other heavy vehicle
    NGA,  // ङ - Power tiller / tractor / special
    CHA,  // च - Private light/medium vehicle (car, jeep, van)
    CHHA, // छ - Rented/commercial light vehicle
    JA,   // ज - Public/commercial light/medium vehicle (taxi, tempo, microbus)
    JHA,  // झ - Government light/medium vehicle
    NYA,  // ञ - Corporation/public enterprise light vehicle
    PA,   // प - Private two-wheeler (motorcycle, scooter)
    PHA,  // फ - Commercial/public two-wheeler
    BA,   // ब - Government two-wheeler
    BHA,  // भ - Customs/special vehicle
    MA,   // म - Special purpose vehicle
    YA,   // य - Tourist vehicle (green plate)
    RA,   // र - State/corporation vehicle
    LA,   // ल - Special/other
    WA,   // व - Special/other
    SHA,  // श - Special/other
    HA,   // ह - E-rickshaw / three-wheeler
    CD,   // Diplomatic vehicle
    UN,   // United Nations vehicle
}

impl FromStr for VehicleCategory {
    type Err = VehicleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "KA" => Ok(Self::KA),
            "KHA" => Ok(Self::KHA),
            "GA" => Ok(Self::GA),
            "GHA" => Ok(Self::GHA),
            "NGA" | "NA" => Ok(Self::NGA),
            "CHA" => Ok(Self::CHA),
            "CHHA" => Ok(Self::CHHA),
            "JA" => Ok(Self::JA),
            "JHA" => Ok(Self::JHA),
            "NYA" => Ok(Self::NYA),
            "PA" => Ok(Self::PA),
            "PHA" | "FA" => Ok(Self::PHA),
            "BA" => Ok(Self::BA),
            "BHA" => Ok(Self::BHA),
            "MA" => Ok(Self::MA),
            "YA" => Ok(Self::YA),
            "RA" => Ok(Self::RA),
            "LA" => Ok(Self::LA),
            "WA" | "VA" => Ok(Self::WA),
            "SHA" | "SA" => Ok(Self::SHA),
            "HA" => Ok(Self::HA),
            "CD" => Ok(Self::CD),
            "UN" => Ok(Self::UN),
            _ => Err(VehicleParseError::InvalidCategory(s.to_string())),
        }
    }
}

impl fmt::Display for VehicleCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Self::KA => "KA",
            Self::KHA => "KHA",
            Self::GA => "GA",
            Self::GHA => "GHA",
            Self::NGA => "NGA",
            Self::CHA => "CHA",
            Self::CHHA => "CHHA",
            Self::JA => "JA",
            Self::JHA => "JHA",
            Self::NYA => "NYA",
            Self::PA => "PA",
            Self::PHA => "PHA",
            Self::BA => "BA",
            Self::BHA => "BHA",
            Self::MA => "MA",
            Self::YA => "YA",
            Self::RA => "RA",
            Self::LA => "LA",
            Self::WA => "WA",
            Self::SHA => "SHA",
            Self::HA => "HA",
            Self::CD => "CD",
            Self::UN => "UN",
        };
        write!(f, "{code}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VehicleNumber {
    pub vehicle_state: VehicleState,
    pub transport_office: Option<TransportOffice>,
    pub group_code: GroupCode,
    pub vehicle_category: VehicleCategory,
    pub registration_number: u32,
}

impl VehicleNumber {
    pub fn new(
        vehicle_state: VehicleState,
        transport_office: Option<TransportOffice>,
        group_code: GroupCode,
        vehicle_category: VehicleCategory,
        registration_number: u32,
    ) -> Self {
        VehicleNumber {
            vehicle_state,
            transport_office,
            group_code,
            vehicle_category,
            registration_number,
        }
    }
}

/// Normalizes a state/zone identifier by removing spaces and hyphens and uppercasing.
/// Returns `None` if any non-ASCII-alphanumeric character (other than space or hyphen) is present.
fn normalize_token(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else if c == '-' || c.is_whitespace() {
            continue;
        } else {
            return None;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Splits the vehicle string into tokens by single hyphens or whitespace,
/// rejecting leading/trailing hyphens or consecutive hyphens (e.g. `"--"`).
fn tokenize_plate(s: &str) -> Result<Vec<&str>, VehicleParseError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(VehicleParseError::EmptyInput);
    }

    if trimmed.starts_with('-') || trimmed.ends_with('-') || trimmed.contains("--") {
        return Err(VehicleParseError::InvalidFormat(s.to_string()));
    }

    let mut tokens = Vec::new();
    for segment in trimmed.split('-') {
        let segment = segment.trim();
        if segment.is_empty() {
            // Happened if there was "- -" or similar empty hyphenated section
            return Err(VehicleParseError::InvalidFormat(s.to_string()));
        }
        for word in segment.split_whitespace() {
            tokens.push(word);
        }
    }

    Ok(tokens)
}

impl FromStr for VehicleNumber {
    type Err = VehicleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let tokens = tokenize_plate(s)?;

        // Minimum 4 tokens (e.g. ["BA", "23", "KHA", "5555"])
        if tokens.len() < 4 {
            return Err(VehicleParseError::InvalidFormat(s.to_string()));
        }

        let len = tokens.len();
        let reg_str = tokens[len - 1];
        let cat_str = tokens[len - 2];
        let group_str = tokens[len - 3];

        // Registration number must be 1..=9999 and at most 4 digits
        if reg_str.len() > 4 || !reg_str.chars().all(|c| c.is_ascii_digit()) {
            return Err(VehicleParseError::InvalidRegistrationNumber(
                reg_str.to_string(),
            ));
        }
        let registration_number = reg_str
            .parse::<u32>()
            .map_err(|_| VehicleParseError::InvalidRegistrationNumber(reg_str.to_string()))?;
        if registration_number == 0 || registration_number > 9999 {
            return Err(VehicleParseError::InvalidRegistrationNumber(
                reg_str.to_string(),
            ));
        }

        let vehicle_category = VehicleCategory::from_str(cat_str)?;
        let group_code = GroupCode::from_str(group_str)?;

        let prefix_tokens = &tokens[..len - 3];
        if prefix_tokens.is_empty() {
            return Err(VehicleParseError::InvalidFormat(s.to_string()));
        }

        // Case 1: Only 1 prefix token (4 tokens total, e.g., "BA-23-KHA-5555").
        // This must be a Zonal plate (provincial plates require a transport office code).
        if prefix_tokens.len() == 1 {
            let state = VehicleState::parse_zonal(prefix_tokens[0])?;
            return Ok(VehicleNumber::new(
                state,
                None,
                group_code,
                vehicle_category,
                registration_number,
            ));
        }

        // Case 2: 2 or more prefix tokens (e.g. ["Province", "4", "15"] or ["Ga", "Pra", "15"] or ["P4", "15"]).
        // The last token of `prefix_tokens` is the transport office code, and the preceding tokens form the provincial state.
        let possible_office_str = prefix_tokens[prefix_tokens.len() - 1];
        let state_part = prefix_tokens[..prefix_tokens.len() - 1].join(" ");

        // Validate the state part first, then the transport office code.
        let state = VehicleState::parse_provincial(&state_part)?;
        let office = TransportOffice::from_str(possible_office_str)?;

        Ok(VehicleNumber::new(
            state,
            Some(office),
            group_code,
            vehicle_category,
            registration_number,
        ))
    }
}

impl TryFrom<&str> for VehicleNumber {
    type Error = VehicleParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        VehicleNumber::from_str(value)
    }
}

impl TryFrom<String> for VehicleNumber {
    type Error = VehicleParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        VehicleNumber::from_str(&value)
    }
}

impl fmt::Display for VehicleNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.transport_office {
            Some(office) => write!(
                f,
                "{}-{}-{}-{}-{}",
                self.vehicle_state,
                office,
                self.group_code,
                self.vehicle_category,
                self.registration_number
            ),
            None => write!(
                f,
                "{}-{}-{}-{}",
                self.vehicle_state,
                self.group_code,
                self.vehicle_category,
                self.registration_number
            ),
        }
    }
}

/// Pure read-only parser helper returning `None` when a plate string is non-standard or unparseable.
pub fn parse_vehicle_number(raw: &str) -> Option<VehicleNumber> {
    VehicleNumber::from_str(raw).ok()
}

/// Returns the canonical plate representation (e.g. `"Province-4-15-23-KHA-5555"` or `"BA-23-KHA-5555"`).
pub fn canonical_vehicle_number(raw: &str) -> Option<String> {
    parse_vehicle_number(raw).map(|vn| vn.to_string())
}

/// Returns an uppercase alphanumeric key derived from the canonical `VehicleNumber`
/// (e.g. `"Ga Pra 15-23-KHA-5555"` -> `"PROVINCE41523KHA5555"`).
pub fn canonical_normalized_key(raw: &str) -> Option<String> {
    canonical_vehicle_number(raw).map(|canon| {
        canon
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect()
    })
}

/// Checks whether two plate strings represent the exact same structured `VehicleNumber`.
pub fn are_equivalent_vehicle_numbers(a: &str, b: &str) -> bool {
    matches!(
        (parse_vehicle_number(a), parse_vehicle_number(b)),
        (Some(va), Some(vb)) if va == vb
    )
}

/// Checks whether two parsed plates represent a Zonal <-> Provincial re-registration transition
/// preserving `group_code`, `vehicle_category`, and `registration_number`.
pub fn is_zone_to_province_transition_parsed(a: &VehicleNumber, b: &VehicleNumber) -> bool {
    let crossed_system = (a.vehicle_state.is_zonal() && b.vehicle_state.is_provincial())
        || (a.vehicle_state.is_provincial() && b.vehicle_state.is_zonal());
    crossed_system
        && a.group_code == b.group_code
        && a.vehicle_category == b.vehicle_category
        && a.registration_number == b.registration_number
}

/// Checks whether two plate strings represent a Zonal <-> Provincial re-registration transition.
pub fn is_zone_to_province_transition(a: &str, b: &str) -> bool {
    matches!(
        (parse_vehicle_number(a), parse_vehicle_number(b)),
        (Some(va), Some(vb)) if is_zone_to_province_transition_parsed(&va, &vb)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_example_plates_and_canonical_equivalence() {
        let examples = [
            // Provincial & State formats for Province 4 (Gandaki)
            "Province-4-15-23-KHA-5555",
            "Ga Pra 15-23-KHA-5555",
            "Ga-Pra-15-23-KHA-5555",
            "Gandaki Pradesh 15 23 KHA 5555",
            "State-4-15-23-KHA-5555",
            "P4-15-23-KHA-5555",
            // Other provinces
            "Ko Pra 01-02-CHA-1234",
            "Province-1-1-2-CHA-1234",
            "Madhesh Pradesh 02-10-PA-9999",
            "Ba Pra 01-025-CHA-4321",
            "Lu Pra 01-001-KHA-7777",
            "Ka Pra 02-001-GA-1111",
            "Su Pa Pra 01-002-PA-8888",
            // Zonal formats
            "Ba 23 KHA 5555",
            "BA-23-KHA-5555",
            "Bagmati 23 KHA 5555",
            "Na 1 KHA 1234",
            "Narayani-1-KHA-1234",
            "Ga 2 CHA 4444",
        ];

        for input in examples {
            assert!(
                VehicleNumber::from_str(input).is_ok(),
                "Expected valid parse for {input}"
            );
        }

        // Verify that Province-4-15-23-KHA-5555 and Ga Pra 15-23-KHA-5555 produce the exact same VehicleNumber & generic string
        let v1: VehicleNumber = "Province-4-15-23-KHA-5555".parse().unwrap();
        let v2: VehicleNumber = "Ga Pra 15-23-KHA-5555".parse().unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v1.to_string(), "Province-4-15-23-KHA-5555");
        assert_eq!(v1.to_string(), v2.to_string());
    }

    #[test]
    fn test_helper_functions_and_zone_province_transition() {
        assert_eq!(
            canonical_vehicle_number("Ga Pra 15-23-KHA-5555").as_deref(),
            Some("Province-4-15-23-KHA-5555")
        );
        assert_eq!(
            canonical_normalized_key("Ga Pra 15-23-KHA-5555").as_deref(),
            Some("PROVINCE41523KHA5555")
        );
        assert_eq!(
            canonical_normalized_key("Bagmati 23 KHA 5555").as_deref(),
            Some("BA23KHA5555")
        );
        assert!(are_equivalent_vehicle_numbers(
            "Province-4-15-23-KHA-5555",
            "Ga Pra 15-23-KHA-5555"
        ));
        assert!(!are_equivalent_vehicle_numbers(
            "Province-4-15-23-KHA-5555",
            "BA-23-KHA-5555"
        ));
        assert!(is_zone_to_province_transition(
            "BA-23-KHA-5555",
            "Province-3-01-23-KHA-5555"
        ));
        assert!(!is_zone_to_province_transition(
            "BA-23-KHA-5555",
            "Province-3-01-99-KHA-5555"
        ));
        assert!(!is_zone_to_province_transition(
            "Province-4-15-23-KHA-5555",
            "Ga Pra 15-23-KHA-5555"
        ));
    }

    #[test]
    fn test_provincial_and_state_equivalence() {
        let inputs = [
            "Province-4-15-23-KHA-5555",
            "Ga Pra 15-23-KHA-5555",
            "Ga-Pra-15-23-KHA-5555",
            "Gandaki Pradesh 15-23-KHA-5555",
            "Gandaki Province 15 23 KHA 5555",
            "State-4-15-23-KHA-5555",
            "Pradesh-4-15-23-KHA-5555",
            "P4-15-23-KHA-5555",
            "ga pra 015-023-kha-5555",
        ];

        let expected = VehicleNumber::new(
            VehicleState::P4,
            Some(TransportOffice(15)),
            GroupCode(23),
            VehicleCategory::KHA,
            5555,
        );

        for input in inputs {
            let parsed: VehicleNumber = input.parse().expect(input);
            assert_eq!(parsed, expected, "Failed struct equality for {input}");
            assert_eq!(
                parsed.to_string(),
                "Province-4-15-23-KHA-5555",
                "Failed canonical string for {input}"
            );
        }
    }

    #[test]
    fn test_all_seven_provinces() {
        let cases = [
            ("Ko Pra 1-2-PA-1000", "Province-1-1-2-PA-1000", VehicleState::P1),
            ("Ma Pra 2-3-KHA-2000", "Province-2-2-3-KHA-2000", VehicleState::P2),
            ("Ba Pra 3-4-CHA-3000", "Province-3-3-4-CHA-3000", VehicleState::P3),
            ("Ga Pra 4-5-GA-4000", "Province-4-4-5-GA-4000", VehicleState::P4),
            ("Lu Pra 5-6-JA-5000", "Province-5-5-6-JA-5000", VehicleState::P5),
            ("Ka Pra 6-7-BA-6000", "Province-6-6-7-BA-6000", VehicleState::P6),
            ("Su Pa Pra 7-8-HA-7000", "Province-7-7-8-HA-7000", VehicleState::P7),
        ];

        for (input, expected_str, expected_state) in cases {
            let parsed: VehicleNumber = input.parse().expect(input);
            assert_eq!(parsed.vehicle_state, expected_state);
            assert_eq!(parsed.to_string(), expected_str);
        }
    }

    #[test]
    fn test_zonal_format_equivalence() {
        let z1: VehicleNumber = "Ba 23 KHA 5555".parse().unwrap();
        let z2: VehicleNumber = "BA-23-KHA-5555".parse().unwrap();
        let z3: VehicleNumber = "Bagmati-23-Kha-5555".parse().unwrap();

        assert_eq!(z1, z2);
        assert_eq!(z2, z3);
        assert_eq!(z1.transport_office, None);
        assert_eq!(z1.vehicle_state, VehicleState::Bagmati);
        assert_eq!(z1.to_string(), "BA-23-KHA-5555");
    }

    #[test]
    fn test_breaking_empty_and_whitespace_inputs() {
        let empty_cases = ["", " ", "   ", "\t", "\n", " \t\n "];
        for input in empty_cases {
            assert_eq!(
                VehicleNumber::from_str(input),
                Err(VehicleParseError::EmptyInput),
                "Expected EmptyInput for {input:?}"
            );
        }
    }

    #[test]
    fn test_breaking_malformed_delimiters_and_segment_counts() {
        let invalid_format_cases = [
            "-",
            "--",
            "---",
            // Leading or trailing hyphens
            "-Province-4-15-23-KHA-5555",
            "Province-4-15-23-KHA-5555-",
            "-BA-23-KHA-5555-",
            // Double/consecutive hyphens or empty hyphenated segments
            "Province--4-15-23-KHA-5555",
            "Province-4--15-23-KHA-5555",
            "Province-4-15-23-KHA--5555",
            "Ga Pra - - 15-23-KHA-5555",
            "BA--23-KHA-5555",
            // Too few segments (< 4 tokens)
            "5555",
            "KHA-5555",
            "23-KHA-5555",
            "Province-4-5555",
            "Ga KHA 5555",
            // No delimiters at all
            "Province41523KHA5555",
            "BA23KHA5555",
        ];

        for input in invalid_format_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidFormat(_))
                ),
                "Expected InvalidFormat for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_invalid_provinces_and_zones() {
        let invalid_state_cases = [
            // Out-of-range province numbers (Nepal only has Provinces 1..=7)
            "Province-0-15-23-KHA-5555",
            "Province-8-15-23-KHA-5555",
            "Province-99-15-23-KHA-5555",
            "Pradesh-0-15-23-KHA-5555",
            "Pradesh-8-15-23-KHA-5555",
            "State-0-15-23-KHA-5555",
            "State-8-15-23-KHA-5555",
            "P0-15-23-KHA-5555",
            "P8-15-23-KHA-5555",
            // Non-existent province/zone names (e.g. cities/districts instead of zones/provinces)
            "Kathmandu-15-23-KHA-5555",
            "Pokhara-23-KHA-5555",
            "Chitwan-23-KHA-5555",
            "Everest-15-23-KHA-5555",
            "XYZ-23-KHA-5555",
            // Purely Zonal codes mistakenly given a 5th transport-office segment
            "BA-15-23-KHA-5555",
            "ME-15-23-KHA-5555",
            "Mechi-15-23-KHA-5555",
            "Narayani-15-23-KHA-5555",
            // Provincial plate missing transport office code (4 tokens instead of 5)
            "Province-4-23-KHA-5555",
            "Ga-Pra-23-KHA-5555",
            "P4-23-KHA-5555",
            // Special characters / punctuation injected into state or zone
            "Ga$Pra-15-23-KHA-5555",
            "Province_4-15-23-KHA-5555",
            "B@A-23-KHA-5555",
            "Ga.Pra-15-23-KHA-5555",
            // Extra numeric token in prefix
            "Province-4-10-15-23-KHA-5555",
        ];

        for input in invalid_state_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidStateOrZone(_))
                ),
                "Expected InvalidStateOrZone for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_invalid_transport_office() {
        let invalid_office_cases = [
            // Zero office code
            "Province-4-0-23-KHA-5555",
            "Province-4-00-23-KHA-5555",
            "Ga Pra 000-23-KHA-5555",
            // Non-numeric office code
            "Province-4-AB-23-KHA-5555",
            "Ga Pra 15A-23-KHA-5555",
            // Over 3 digits / u16 overflow
            "Province-4-1000-23-KHA-5555",
            "Province-4-99999-23-KHA-5555",
        ];

        for input in invalid_office_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidTransportOffice(_))
                ),
                "Expected InvalidTransportOffice for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_invalid_group_code() {
        let invalid_group_cases = [
            // Zero group/lot code
            "Province-4-15-0-KHA-5555",
            "Province-4-15-00-KHA-5555",
            "BA-0-KHA-5555",
            // Non-numeric group/lot code (including missing numbers after 2-word province prefix)
            "Province-4-15-XY-KHA-5555",
            "Ga Pra 15-23B-KHA-5555",
            "Ga Pra KHA 5555",
            "BA-LOT-KHA-5555",
            // Over 3 digits / u16 overflow
            "Province-4-15-1000-KHA-5555",
            "BA-70000-KHA-5555",
        ];

        for input in invalid_group_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidGroupCode(_))
                ),
                "Expected InvalidGroupCode for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_invalid_vehicle_category() {
        let invalid_category_cases = [
            // Unknown letter combinations
            "Province-4-15-23-XYZ-5555",
            "Province-4-15-23-K-5555",
            "Province-4-15-23-KHAA-5555",
            "Ga Pra 15-23-ZZ-5555",
            "BA-23-Q-5555",
            // Numeric category
            "Province-4-15-23-12-5555",
            "BA-23-99-5555",
            // Category with punctuation
            "Province-4-15-23-KH@-5555",
        ];

        for input in invalid_category_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidCategory(_))
                ),
                "Expected InvalidCategory for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_invalid_registration_number() {
        let invalid_reg_cases = [
            // Zero registration number (plates start at 1)
            "Province-4-15-23-KHA-0",
            "Province-4-15-23-KHA-0000",
            "BA-23-KHA-0",
            // More than 4 digits (> 9999 or 5-digit zero-padded)
            "Province-4-15-23-KHA-10000",
            "Province-4-15-23-KHA-05555",
            "Province-4-15-23-KHA-99999999999",
            "BA-23-KHA-12345",
            // Non-numeric / alphanumeric registration numbers
            "Province-4-15-23-KHA-55A5",
            "Province-4-15-23-KHA-ABCD",
            "BA-23-KHA-55.5",
            "BA-23-KHA-+555",
        ];

        for input in invalid_reg_cases {
            assert!(
                matches!(
                    VehicleNumber::from_str(input),
                    Err(VehicleParseError::InvalidRegistrationNumber(_))
                ),
                "Expected InvalidRegistrationNumber for {input:?}, got {:?}",
                VehicleNumber::from_str(input)
            );
        }
    }

    #[test]
    fn test_breaking_swapped_component_order() {
        // Swapped category and group code: "KHA" is in group position and "23" is in category position
        assert!(matches!(
            VehicleNumber::from_str("Province-4-15-KHA-23-5555"),
            Err(VehicleParseError::InvalidCategory(_))
        ));

        // Reversed plate order
        assert!(VehicleNumber::from_str("5555-KHA-23-15-Province-4").is_err());
        assert!(VehicleNumber::from_str("5555-KHA-23-BA").is_err());
    }
}
