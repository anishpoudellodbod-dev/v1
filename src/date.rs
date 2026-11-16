#[derive(Debug, PartialEq, Clone, Copy, Eq, serde::Serialize, serde::Deserialize)]
pub enum DateType{
    AD,
    BS
}

#[derive(Debug, PartialEq, Clone, Copy, Eq, serde::Serialize, serde::Deserialize)]
pub struct Date{
    pub date: DateParams,
    pub date_type: DateType
}

#[derive(Debug, PartialEq, Clone, Copy, Eq, serde::Serialize, serde::Deserialize)]
pub struct DateParams{
    pub year:i32,
    pub month:i32,
    pub day:i32
}

#[derive(Debug, PartialEq, Clone, Copy, Eq, serde::Serialize, serde::Deserialize)]
pub struct DateRange{
    pub start: Date,
    pub end: Date
}

pub fn check_ad_bs(date: DateParams) -> DateType {
    
    if date.year > 2050 && date.year < 2090 {
        DateType::BS
    } else if date.year > 1950 && date.year < 2050{
        DateType::AD
    } else {
        panic!("Year {} falls outside the valid range of 1950-2090", date.year);
    }

}

impl std::fmt::Display for DateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DateType::AD => write!(f, "AD"),
            DateType::BS => write!(f, "BS"),
        }
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{} {}", self.date.year, self.date.month, self.date.day, self.date_type)
    }
}

impl From<DateParams> for Date{
    fn from(date: DateParams) -> Self {
        Date{
            date,
            date_type: check_ad_bs(date)
        }
    }
}

impl From<String> for DateParams{
    fn from(date: String) -> Self {
        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() != 3 {
            panic!("Invalid date format");
        }
        let year = parts[0].parse::<i32>().expect("Invalid year");
        let month = parts[1].parse::<i32>().expect("Invalid month");
        let day = parts[2].parse::<i32>().expect("Invalid day");

        if month < 1 || month > 12 {
            panic!("Invalid month {} falls outside the valid range of 1-12", month);
        }
        if day < 1 || day > 32 {
            panic!("Invalid day {} falls outside the valid range of 1-32", day);
        }

        DateParams{
            year,
            month,
            day
        }
    }
}

impl From<&str> for DateParams {
    fn from(date: &str) -> Self {
        DateParams::from(date.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DateMapper {
    /// A map from AD dates to BS dates
    pub ad_to_bs: std::collections::HashMap<String, String>,
    /// A map from BS dates to AD dates
    pub bs_to_ad: std::collections::HashMap<String, String>,
    pub bs: std::collections::HashMap<String, String>,
    pub ad: std::collections::HashMap<String, String>,
}

impl Default for DateMapper {
    fn default() -> Self {
        Self::empty()
    }
}

impl DateMapper {
    pub fn empty() -> Self {
        DateMapper {
            ad_to_bs: std::collections::HashMap::new(),
            bs_to_ad: std::collections::HashMap::new(),
            bs: std::collections::HashMap::new(),
            ad: std::collections::HashMap::new(),
        }
    }

    pub fn new() -> Self {
        DateMapper::empty().load_from_json_file("date_mappings.json").unwrap_or_else(|_| {
            panic!("Failed to load date mappings from JSON file");
        })

    }
    /// Load date mappings from a JSON file.
    /*
        Sample JSON structure for date mapping:
        [{
        "ad_date": "1975-01-01",
        "bs_date": "2031-09-17"
        }
        ...]
    */
    pub fn load_from_json_file(
        &mut self,
        file_path: &str,
    ) -> Result<DateMapper, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(file_path)?;
        let reader = std::io::BufReader::new(file);
        self.load_from_reader(reader)
    }

    pub fn load_from_reader<R: std::io::Read>(
        &mut self,
        reader: R,
    ) -> Result<DateMapper, Box<dyn std::error::Error>> {
        let date_mappings: Vec<std::collections::HashMap<String, String>> =
            serde_json::from_reader(reader)?;

        for mapping in date_mappings {
            if let (Some(ad_date), Some(bs_date)) = (mapping.get("ad_date"), mapping.get("bs_date")) {
                self.ad_to_bs.insert(ad_date.clone(), bs_date.clone());
                self.bs_to_ad.insert(bs_date.clone(), ad_date.clone());
                self.ad.insert(ad_date.clone(), bs_date.clone());
                self.bs.insert(bs_date.clone(), ad_date.clone());
            }
        }

        Ok(self.clone())
    }

    pub fn load_from_json_str(
        &mut self,
        json_str: &str,
    ) -> Result<DateMapper, Box<dyn std::error::Error>> {
        self.load_from_reader(json_str.as_bytes())
    }
}

impl DateParams {
    pub fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    pub fn days_in_month(year: i32, month: i32) -> i32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => if Self::is_leap_year(year) { 29 } else { 28 },
            _ => 30,
        }
    }

    pub fn to_date_string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    //Get End Of Month Date from DateMapper
    pub fn get_end_of_month(&self, date_mapper: &DateMapper) -> Date {
        //Load the date mappings from the DateMapper
        let date_mappings = &date_mapper.ad_to_bs;
        //Get the end of month date from the date mappings
        let month = self.month;
        let year = self.year;
        let day = self.day;
        //Get the end of month date from the date mappings
        let mut end_of_month_date_str = format!("{}-{}-{}", year, month, day);
        if let Some(end_of_month_date) = date_mappings.get(&end_of_month_date_str) {
            end_of_month_date_str = end_of_month_date.clone();
        } else {
            panic!("End of month date not found in date mappings for date: {}", end_of_month_date_str);
        }
        //Parse the end of month date string to DateParams
        let end_of_month_date_params: DateParams = end_of_month_date_str.into();
        //Return the end of month date as Date
        Date {
            date: end_of_month_date_params,
            date_type: check_ad_bs(end_of_month_date_params),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // 1. Tests for check_ad_bs
    // =========================================================================

    #[test]
    fn test_check_ad_bs_ad_valid() {
        let years = [1952, 1970, 2000, 2024, 2048];
        for year in years {
            let params = DateParams { year, month: 1, day: 1 };
            assert_eq!(check_ad_bs(params), DateType::AD);
        }
    }

    #[test]
    fn test_check_ad_bs_bs_valid() {
        let years = [2052, 2060, 2075, 2080, 2085, 2088];
        for year in years {
            let params = DateParams { year, month: 1, day: 1 };
            assert_eq!(check_ad_bs(params), DateType::BS);
        }
    }

    // Edge cases for check_ad_bs: boundary values within valid range
    #[test]
    fn test_check_ad_bs_edge_min_ad() {
        // 1951 is the lowest year where year > 1950
        let params = DateParams { year: 1951, month: 6, day: 15 };
        assert_eq!(check_ad_bs(params), DateType::AD);
    }

    #[test]
    fn test_check_ad_bs_edge_max_ad() {
        // 2049 is the highest year where year < 2050
        let params = DateParams { year: 2049, month: 12, day: 31 };
        assert_eq!(check_ad_bs(params), DateType::AD);
    }

    #[test]
    fn test_check_ad_bs_edge_min_bs() {
        // 2051 is the lowest year where year > 2050
        let params = DateParams { year: 2051, month: 1, day: 1 };
        assert_eq!(check_ad_bs(params), DateType::BS);
    }

    #[test]
    fn test_check_ad_bs_edge_max_bs() {
        // 2089 is the highest year where year < 2090
        let params = DateParams { year: 2089, month: 12, day: 32 };
        assert_eq!(check_ad_bs(params), DateType::BS);
    }

    // Boundary edge cases that must panic
    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_lower_bound_1950() {
        // Year 1950 is strictly excluded by `year > 1950`
        check_ad_bs(DateParams { year: 1950, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_boundary_gap_2050() {
        // Year 2050 is not > 2050 and not < 2050, falling through to panic
        check_ad_bs(DateParams { year: 2050, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_upper_bound_2090() {
        // Year 2090 is strictly excluded by `year < 2090`
        check_ad_bs(DateParams { year: 2090, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_below_1950() {
        check_ad_bs(DateParams { year: 1949, month: 12, day: 31 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_above_2090() {
        check_ad_bs(DateParams { year: 2091, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_zero_year() {
        check_ad_bs(DateParams { year: 0, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_negative_year() {
        check_ad_bs(DateParams { year: -100, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_i32_min() {
        check_ad_bs(DateParams { year: i32::MIN, month: 1, day: 1 });
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_check_ad_bs_edge_panic_i32_max() {
        check_ad_bs(DateParams { year: i32::MAX, month: 1, day: 1 });
    }

    // =========================================================================
    // 2. Tests for DateParams parsing (From<String> and From<&str>)
    // =========================================================================

    #[test]
    fn test_date_params_from_string_valid() {
        let params: DateParams = "2024-05-15".to_string().into();
        assert_eq!(params, DateParams { year: 2024, month: 5, day: 15 });

        let params2 = DateParams::from("1999-12-31");
        assert_eq!(params2, DateParams { year: 1999, month: 12, day: 31 });
    }

    #[test]
    fn test_date_params_single_digit_month_and_day() {
        let params = DateParams::from("2024-1-9");
        assert_eq!(params, DateParams { year: 2024, month: 1, day: 9 });
    }

    #[test]
    fn test_date_params_edge_min_month_and_day() {
        let params = DateParams::from("2024-01-01");
        assert_eq!(params.month, 1);
        assert_eq!(params.day, 1);
    }

    #[test]
    fn test_date_params_edge_max_month() {
        let params = DateParams::from("2024-12-25");
        assert_eq!(params.month, 12);
    }

    #[test]
    fn test_date_params_edge_max_day_32() {
        // BS calendar months can have up to 32 days
        let params = DateParams::from("2081-01-32");
        assert_eq!(params.day, 32);
    }

    // DateParams parsing edge cases that should panic
    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_empty_string_panics() {
        let _ = DateParams::from("");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_too_few_parts_panics() {
        let _ = DateParams::from("2024-05");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_too_many_parts_panics() {
        let _ = DateParams::from("2024-05-15-01");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_slash_delimiter_panics() {
        let _ = DateParams::from("2024/05/15");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_dot_delimiter_panics() {
        let _ = DateParams::from("2024.05.15");
    }

    #[test]
    #[should_panic(expected = "Invalid year")]
    fn test_date_params_non_numeric_year_panics() {
        let _ = DateParams::from("year-05-15");
    }

    #[test]
    #[should_panic(expected = "Invalid month")]
    fn test_date_params_non_numeric_month_panics() {
        let _ = DateParams::from("2024-may-15");
    }

    #[test]
    #[should_panic(expected = "Invalid day")]
    fn test_date_params_non_numeric_day_panics() {
        let _ = DateParams::from("2024-05-fifteen");
    }

    #[test]
    #[should_panic(expected = "Invalid month 0 falls outside the valid range of 1-12")]
    fn test_date_params_month_zero_panics() {
        let _ = DateParams::from("2024-00-15");
    }

    #[test]
    #[should_panic(expected = "Invalid month 13 falls outside the valid range of 1-12")]
    fn test_date_params_month_thirteen_panics() {
        let _ = DateParams::from("2024-13-15");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_negative_month_double_hyphen_panics() {
        // "2024--1-15" splits into 4 parts due to double hyphen
        let _ = DateParams::from("2024--1-15");
    }

    #[test]
    #[should_panic(expected = "Invalid day 0 falls outside the valid range of 1-32")]
    fn test_date_params_day_zero_panics() {
        let _ = DateParams::from("2024-05-00");
    }

    #[test]
    #[should_panic(expected = "Invalid day 33 falls outside the valid range of 1-32")]
    fn test_date_params_day_thirty_three_panics() {
        let _ = DateParams::from("2024-05-33");
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_date_params_negative_day_double_hyphen_panics() {
        // "2024-05--5" splits into 4 parts due to double hyphen
        let _ = DateParams::from("2024-05--5");
    }

    // =========================================================================
    // 3. Tests for Date construction, Display, and traits
    // =========================================================================

    #[test]
    fn test_date_from_date_params_ad() {
        let params = DateParams { year: 2024, month: 10, day: 29 };
        let date = Date::from(params);
        assert_eq!(date.date, params);
        assert_eq!(date.date_type, DateType::AD);
    }

    #[test]
    fn test_date_from_date_params_bs() {
        let params = DateParams { year: 2081, month: 7, day: 13 };
        let date = Date::from(params);
        assert_eq!(date.date, params);
        assert_eq!(date.date_type, DateType::BS);
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_date_from_date_params_invalid_year_panics() {
        let params = DateParams { year: 2050, month: 1, day: 1 };
        let _ = Date::from(params);
    }

    #[test]
    fn test_date_display_and_to_string() {
        let ad_date = Date {
            date: DateParams { year: 2024, month: 5, day: 15 },
            date_type: DateType::AD,
        };
        assert_eq!(ad_date.to_string(), "2024-5-15 AD");
        assert_eq!(format!("{}", ad_date), "2024-5-15 AD");

        let bs_date = Date {
            date: DateParams { year: 2081, month: 1, day: 30 },
            date_type: DateType::BS,
        };
        assert_eq!(bs_date.to_string(), "2081-1-30 BS");
        assert_eq!(format!("{}", bs_date), "2081-1-30 BS");
    }

    #[test]
    fn test_date_type_display() {
        assert_eq!(DateType::AD.to_string(), "AD");
        assert_eq!(DateType::BS.to_string(), "BS");
        assert_eq!(format!("{}", DateType::AD), "AD");
        assert_eq!(format!("{}", DateType::BS), "BS");
    }

    #[test]
    fn test_date_clone_copy_equality() {
        let d1 = Date {
            date: DateParams { year: 2024, month: 1, day: 1 },
            date_type: DateType::AD,
        };
        let d2 = d1; // Copy
        assert_eq!(d1, d2);
        assert_eq!(d1.clone(), d2);

        let d3 = Date {
            date: DateParams { year: 2024, month: 1, day: 2 },
            date_type: DateType::AD,
        };
        assert_ne!(d1, d3);
    }

    // =========================================================================
    // 4. Tests for DateRange
    // =========================================================================

    #[test]
    fn test_date_range_valid() {
        let start = Date::from(DateParams { year: 2024, month: 1, day: 1 });
        let end = Date::from(DateParams { year: 2024, month: 12, day: 31 });
        let range = DateRange { start, end };

        assert_eq!(range.start, start);
        assert_eq!(range.end, end);

        let range_copy = range;
        assert_eq!(range, range_copy);
    }

    #[test]
    fn test_date_range_same_start_and_end() {
        let d = Date::from(DateParams { year: 2024, month: 6, day: 15 });
        let range = DateRange { start: d, end: d };
        assert_eq!(range.start, range.end);
    }

    #[test]
    fn test_date_range_cross_calendar() {
        let ad_start = Date::from(DateParams { year: 2024, month: 1, day: 1 });
        let bs_end = Date::from(DateParams { year: 2081, month: 1, day: 1 });
        let range = DateRange { start: ad_start, end: bs_end };
        assert_eq!(range.start.date_type, DateType::AD);
        assert_eq!(range.end.date_type, DateType::BS);
    }

    // =========================================================================
    // 5. Tests for DateMapper
    // =========================================================================

    #[test]
    fn test_date_mapper_empty_and_default() {
        let mapper = DateMapper::empty();
        assert!(mapper.ad_to_bs.is_empty());
        assert!(mapper.bs_to_ad.is_empty());
        assert!(mapper.ad.is_empty());
        assert!(mapper.bs.is_empty());

        let default_mapper = DateMapper::default();
        assert_eq!(mapper, default_mapper);
    }

    #[test]
    fn test_date_mapper_load_from_json_str_valid() {
        let json_data = r#"[
            {
                "ad_date": "1975-01-01",
                "bs_date": "2031-09-17"
            },
            {
                "ad_date": "2024-05-15",
                "bs_date": "2081-02-02"
            }
        ]"#;

        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_str(json_data);
        assert!(result.is_ok());

        assert_eq!(mapper.ad_to_bs.get("1975-01-01"), Some(&"2031-09-17".to_string()));
        assert_eq!(mapper.bs_to_ad.get("2031-09-17"), Some(&"1975-01-01".to_string()));
        assert_eq!(mapper.ad_to_bs.get("2024-05-15"), Some(&"2081-02-02".to_string()));
        assert_eq!(mapper.bs_to_ad.get("2081-02-02"), Some(&"2024-05-15".to_string()));
    }

    #[test]
    fn test_date_mapper_load_from_json_str_empty_array() {
        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_str("[]");
        assert!(result.is_ok());
        assert!(mapper.ad_to_bs.is_empty());
        assert!(mapper.bs_to_ad.is_empty());
    }

    #[test]
    fn test_date_mapper_load_from_json_str_missing_fields_skipped() {
        let json_data = r#"[
            {
                "ad_date": "2024-01-01"
            },
            {
                "bs_date": "2080-09-17"
            },
            {
                "ad_date": "2024-05-15",
                "bs_date": "2081-02-02"
            }
        ]"#;

        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_str(json_data);
        assert!(result.is_ok());
        // Only the entry with both fields should be inserted
        assert_eq!(mapper.ad_to_bs.len(), 1);
        assert_eq!(mapper.ad_to_bs.get("2024-05-15"), Some(&"2081-02-02".to_string()));
    }

    #[test]
    fn test_date_mapper_load_from_json_str_malformed_syntax() {
        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_str("not a json string");
        assert!(result.is_err());
    }

    #[test]
    fn test_date_mapper_load_from_file_not_found() {
        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_file("non_existent_file_12345.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_date_mapper_load_from_file_success() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_date_mappings.json");
        let json_content = r#"[{"ad_date":"2024-01-01","bs_date":"2080-09-17"}]"#;

        let mut file = std::fs::File::create(&file_path).expect("Failed to create temp file");
        file.write_all(json_content.as_bytes()).expect("Failed to write to temp file");

        let mut mapper = DateMapper::empty();
        let result = mapper.load_from_json_file(file_path.to_str().unwrap());
        assert!(result.is_ok());
        assert_eq!(mapper.ad_to_bs.get("2024-01-01"), Some(&"2080-09-17".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    #[should_panic(expected = "Failed to load date mappings from JSON file")]
    fn test_date_mapper_new_panics_on_missing_file() {
        // DateMapper::new() tries to load "date_mappings.json" which does not exist in root
        let _ = DateMapper::new();
    }

    // =========================================================================
    // 6. Tests for get_end_of_month
    // =========================================================================

    #[test]
    fn test_get_end_of_month_success() {
        let mut mapper = DateMapper::empty();
        // DateParams formats search string as "{year}-{month}-{day}"
        mapper.ad_to_bs.insert("2024-1-31".to_string(), "2080-10-17".to_string());

        let date_params = DateParams { year: 2024, month: 1, day: 31 };
        let end_of_month = date_params.get_end_of_month(&mapper);

        assert_eq!(end_of_month.date.year, 2080);
        assert_eq!(end_of_month.date.month, 10);
        assert_eq!(end_of_month.date.day, 17);
        assert_eq!(end_of_month.date_type, DateType::BS);
    }

    #[test]
    #[should_panic(expected = "End of month date not found in date mappings")]
    fn test_get_end_of_month_not_found_panics() {
        let mapper = DateMapper::empty();
        let date_params = DateParams { year: 2024, month: 2, day: 28 };
        date_params.get_end_of_month(&mapper);
    }

    #[test]
    #[should_panic(expected = "falls outside the valid range of 1950-2090")]
    fn test_get_end_of_month_mapped_date_out_of_range_panics() {
        let mut mapper = DateMapper::empty();
        // Map to year 2050 which triggers panic in check_ad_bs
        mapper.ad_to_bs.insert("2024-1-1".to_string(), "2050-01-01".to_string());

        let date_params = DateParams { year: 2024, month: 1, day: 1 };
        date_params.get_end_of_month(&mapper);
    }

    #[test]
    #[should_panic(expected = "Invalid date format")]
    fn test_get_end_of_month_mapped_date_invalid_format_panics() {
        let mut mapper = DateMapper::empty();
        mapper.ad_to_bs.insert("2024-1-1".to_string(), "invalid-date".to_string());

        let date_params = DateParams { year: 2024, month: 1, day: 1 };
        date_params.get_end_of_month(&mapper);
    }
}
