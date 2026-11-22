/*
This is a sample of Bill Resource with id 149
{
    "id": 149,
    "resource_name": "BillResource",
    "inner_content": {
      "note": "",
      "items": [
        {
          "title": "Entry",
          "amount": 500,
          "ruleId": "30",
          "feeHeadId": "29",
          "billingEntity": "नारायणी यातायात व्यवसायी संघ",
          "billingEntityId": "1"
        }
      ],
      "total": 565,
      "branch": {
        "id": "1",
        "name": "Main Branch",
        "phone": "057-522338",
        "address": "Hetauda-4,Parijatpath",
        "BranchCode": "MAIN",
        "contactPerson": ""
      },
      "paidAt": "2026-11-17T09:56:28.186Z",
      "vehicle": {
        "id": "36010",
        "phone": "9803924000",
        "address": "Bagmati Province (P3), Lalitpur, Mahalaxmi, Mahalaxmi",
        "smsPhone": "9803924000",
        "ownerName": "akash  shrestha",
        "vehicleType": "22 dimensional",
        "operatorName": "ggsgwgfgf",
        "vehicleNumber": "PROVINCE-4-56-02-JHA-8241"
      },
      "discount": 0,
      "subtotal": 500,
      "createdBy": {
        "id": "3",
        "name": "System Administrator",
        "username": "admin@narayanisangh.com"
      },
      "periodEnd": "2026-12-15",
      "vatAmount": 65,
      "vatRatePct": 13,
      "billingDate": "2026-11-17",
      "billingType": "MEMBERSHIP",
      "periodStart": "2026-11-17",
      "fiscalYearId": "15f21721-50e8-41af-9596-9d8cdb8d6a16",
      "billingMonths": 0,
      "paymentMethod": "नगद",
      "vatBillingEntityIds": [
        "1"
      ]
    },
    "external_content": {
      "createdTime": "2026-11-17T09:55:09Z"
    }
  }
  This is all of BillingRuleResource:
  [
  {
    "id": 29,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "Entry",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 11,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 16,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 15,
        "name": "विस्थापित सहयोग"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Pass bhai sakeko ho",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 12,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 2000,
      "period": "MONTH",
      "status": "ACTIVE",
      "feeHead": {
        "id": 11,
        "name": "यातायात मजदुर दुर्घटना बिमाff"
      },
      "validity": "SAME_DAY",
      "frequency": "PERIODIC",
      "fiscalYear": "2082/83",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-02",
      "billingEntityId": 1,
      "meetingReference": "2083/7/15 ghjiufd",
      "vehicleCategoryId": 1
    },
    "external_content": null
  },
  {
    "id": 13,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "विपद सहयोग कोष",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 3,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 24,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 23,
        "name": "तेश्रो पक्ष दुर्घटना सेवा सहयोग"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 3,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "2",
      "name": "सदस्यता नविकरण शूल्क",
      "status": "ACTIVE",
      "fiscalYear": "2082/83",
      "displayOrder": 1,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 5,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 600,
      "period": "MONTH",
      "status": "ACTIVE",
      "feeHead": {
        "id": 3,
        "name": "सदस्यता नविकरण शूल्क"
      },
      "validity": "END_OF_MONTH",
      "frequency": "PERIODIC",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-10-29",
      "billingEntityId": 1,
      "meetingReference": "2083/7/15 ghjiufd",
      "vehicleCategoryId": 3
    },
    "external_content": null
  },
  {
    "id": 30,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 500,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 29,
        "name": "Entry"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "cvbnm",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 34,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 33,
        "name": "test"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "3456",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 39,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 800,
      "period": "MONTH",
      "status": "ACTIVE",
      "feeHead": {
        "id": 3,
        "name": "सदस्यता नविकरण शूल्क"
      },
      "validity": "END_OF_MONTH",
      "frequency": "PERIODIC",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-02",
      "billingEntityId": 1,
      "meetingReference": "2083/7/15 ghjiufd",
      "vehicleCategoryId": 1
    },
    "external_content": {
      "createdTime": "2026-11-16T09:49:46Z"
    }
  },
  {
    "id": 28,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 27,
        "name": "hello"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Pass bhai sakeko ho",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 9,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "DevTestSulkaSirsak",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 2,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 38,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 490,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 3,
        "name": "सदस्यता नविकरण शूल्क"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-03",
      "billingEntityId": 1,
      "meetingReference": "Meeting House",
      "vehicleCategoryId": 6
    },
    "external_content": null
  },
  {
    "id": 33,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "test",
      "status": "ACTIVE",
      "fiscalYear": "2082/83",
      "displayOrder": 12,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 20,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 19,
        "name": "बिलम्ब बापत"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 17,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "धरौटी सापट",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 5,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 23,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "तेश्रो पक्ष दुर्घटना सेवा सहयोग",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 8,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 27,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "hello",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 10,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 4,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 500,
      "period": "MONTH",
      "status": "ACTIVE",
      "feeHead": {
        "id": 3,
        "name": "सदस्यता नविकरण शूल्क"
      },
      "validity": "END_OF_MONTH",
      "frequency": "PERIODIC",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-10-29",
      "billingEntityId": 1,
      "meetingReference": "2083/7/15 ghjiufd",
      "vehicleCategoryId": 2
    },
    "external_content": null
  },
  {
    "id": 10,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 100,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 9,
        "name": "DevTestSulkaSirsak"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-10-30",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 18,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 17,
        "name": "धरौटी सापट"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 14,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 13,
        "name": "विपद सहयोग कोष"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/83",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-02",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 22,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 500,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 21,
        "name": "नामसरी तथा नम्बर परीवर्तन वापत"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Pass bhai sakeko ho",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 11,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "यातायात मजदुर दुर्घटना बिमाff",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 0,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 25,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "दुर्घटना सहयोग",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 9,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 37,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 36,
        "name": "testing"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/83",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-02",
      "billingEntityId": 1,
      "meetingReference": "67767",
      "vehicleCategoryId": null
    },
    "external_content": null
  },
  {
    "id": 19,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "बिलम्ब बापत",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 6,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 21,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "नामसरी तथा नम्बर परीवर्तन वापत",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 7,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 36,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "testing",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 13,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 40,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "VEHICLE_TYPE",
      "amount": 555,
      "period": "FISCAL_YEAR",
      "status": "ACTIVE",
      "feeHead": {
        "id": 11,
        "name": "यातायात मजदुर दुर्घटना बिमाff"
      },
      "validity": "END_OF_FISCAL_YEAR",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-03",
      "billingEntityId": 1,
      "meetingReference": "Meeting House",
      "vehicleCategoryId": 6
    },
    "external_content": {
      "createdTime": "2026-11-16T10:11:27Z"
    }
  },
  {
    "id": 15,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "code": "",
      "name": "विस्थापित सहयोग",
      "status": "ACTIVE",
      "fiscalYear": "2082/2083",
      "displayOrder": 4,
      "resourceType": "FEE_HEAD"
    },
    "external_content": null
  },
  {
    "id": 26,
    "resource_name": "BillingRuleResource",
    "inner_content": {
      "scope": "COMMON",
      "amount": 0,
      "period": null,
      "status": "ACTIVE",
      "feeHead": {
        "id": 25,
        "name": "दुर्घटना सहयोग"
      },
      "validity": "SAME_DAY",
      "frequency": "ONE_TIME",
      "fiscalYear": "2082/2083",
      "resourceType": "BILLING_RULE",
      "effectiveDate": "2026-11-01",
      "billingEntityId": 1,
      "meetingReference": "Testing Rule",
      "vehicleCategoryId": null
    },
    "external_content": null
  }
]
   */