//! The mutations and queries of customers and companies, through queries over the demo store:
//! for each mutation, what it changes, its payload, its events, and its main user errors (which
//! change nothing).

use serde_json::{Value, json};

use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::{Clock, ShopState};

fn demo() -> (lsf_core::Store, ShopState) {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut state = ShopState::seeded(&store, None);
    // The demo store runs on the system clock: a frozen one gives every change the same time.
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z").unwrap();
    state.set_clock(Clock::Frozen(now.with_timezone(&chrono::Utc)));
    (store, state)
}

/// The answer to a query with variables, `extensions` without `cost`.
fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The payload of the mutation `name` in an answer, which must have no GraphQL error.
fn data<'a>(answer: &'a Value, name: &str) -> &'a Value {
    assert!(answer.get("errors").is_none(), "{answer}");
    &answer["data"][name]
}

/// The topics of the events emitted since the last call.
fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}

/// The GID of a demo customer, by email.
fn customer(shop: &ShopState, email: &str) -> String {
    shop.keys("Customer")
        .find(|key| shop.get(key).unwrap()["email"] == json!(email))
        .unwrap_or_else(|| panic!("no customer {email}"))
        .to_string()
}

const CUSTOMER_FIELDS: &str = "id displayName firstName lastName email phone state locale tags note
    verifiedEmail canDelete numberOfOrders amountSpent { amount currencyCode }
    emailMarketingConsent { marketingState marketingOptInLevel consentUpdatedAt }
    smsMarketingConsent { marketingState marketingOptInLevel consentCollectedFrom }
    defaultEmailAddress { emailAddress marketingState }
    defaultPhoneNumber { phoneNumber smsMarketingConsent { state } }
    defaultAddress { id address1 city province provinceCode country countryCodeV2 formattedArea }
    addressesV2(first: 5) { nodes { id address1 } }
    metafield(namespace: \"custom\", key: \"tier\") { value type }";

#[test]
fn customer_create_update_and_delete() {
    let (_, mut shop) = demo();
    let now = shop.timestamp();
    let created = run(
        &mut shop,
        &format!(
            "mutation($input: CustomerInput!) {{ customerCreate(input: $input) {{
                customer {{ {CUSTOMER_FIELDS} }} userErrors {{ field message }} }} }}"
        ),
        json!({"input": {
            "email": "steve@example.com", "phone": "+1 (646) 555-5555", "firstName": "Steve",
            "lastName": "Lastnameson", "tags": ["vip, new"], "note": "Met at the fair",
            "emailMarketingConsent": {"marketingState": "SUBSCRIBED", "marketingOptInLevel": "CONFIRMED_OPT_IN"},
            "addresses": [{"address1": "412 fake st", "city": "Ottawa", "provinceCode": "ON", "countryCode": "CA", "zip": "A1A 4A1"}],
            "metafields": [{"namespace": "custom", "key": "tier", "type": "single_line_text_field", "value": "gold"}],
        }}),
    );
    let payload = data(&created, "customerCreate");
    assert_eq!(payload["userErrors"], json!([]));
    let steve = &payload["customer"];
    let id = steve["id"].as_str().unwrap().to_string();
    assert_eq!(steve["displayName"], json!("Steve Lastnameson"));
    assert_eq!(steve["phone"], json!("+16465555555"));
    assert_eq!(steve["state"], json!("DISABLED"));
    assert_eq!(steve["verifiedEmail"], json!(true));
    assert_eq!(steve["tags"], json!(["vip", "new"]));
    assert_eq!(steve["note"], json!("Met at the fair"));
    assert_eq!(steve["canDelete"], json!(true));
    assert_eq!(steve["numberOfOrders"], json!("0"));
    assert_eq!(steve["amountSpent"]["amount"], json!("0.0"));
    assert_eq!(
        steve["emailMarketingConsent"],
        json!({"marketingState": "SUBSCRIBED", "marketingOptInLevel": "CONFIRMED_OPT_IN", "consentUpdatedAt": now})
    );
    assert_eq!(
        steve["smsMarketingConsent"],
        json!({"marketingState": "NOT_SUBSCRIBED", "marketingOptInLevel": "SINGLE_OPT_IN", "consentCollectedFrom": "OTHER"})
    );
    assert_eq!(
        steve["defaultEmailAddress"],
        json!({"emailAddress": "steve@example.com", "marketingState": "SUBSCRIBED"})
    );
    assert_eq!(
        steve["defaultPhoneNumber"],
        json!({"phoneNumber": "+16465555555", "smsMarketingConsent": {"state": "NEVER_SUBSCRIBED"}})
    );
    let address = &steve["defaultAddress"];
    assert!(
        address["id"]
            .as_str()
            .unwrap()
            .ends_with("?model_name=CustomerAddress")
    );
    assert_eq!(address["country"], json!("Canada"));
    assert_eq!(address["countryCodeV2"], json!("CA"));
    assert_eq!(address["formattedArea"], json!("Ottawa, ON, Canada"));
    assert_eq!(steve["addressesV2"]["nodes"][0]["id"], address["id"]);
    assert_eq!(
        steve["metafield"],
        json!({"value": "gold", "type": "single_line_text_field"})
    );
    assert_eq!(
        topics(&mut shop),
        ["customers/create", "customer.tags_added"]
    );

    // What is wrong changes nothing.
    let before = shop.count("Customer");
    let create = |shop: &mut ShopState, input: Value| {
        let answer = run(
            shop,
            "mutation($input: CustomerInput!) { customerCreate(input: $input) { customer { id } userErrors { field message } } }",
            json!({"input": input}),
        );
        data(&answer, "customerCreate").clone()
    };
    assert_eq!(
        create(&mut shop, json!({"email": "JANE.DOE@example.com"})),
        json!({"customer": null, "userErrors": [{"field": ["email"], "message": "Email has already been taken"}]})
    );
    assert_eq!(
        create(&mut shop, json!({"email": "jane"}))["userErrors"],
        json!([{"field": ["email"], "message": "Email is invalid"}])
    );
    assert_eq!(
        create(
            &mut shop,
            json!({"firstName": "Bo", "phone": "+1 (646) 555-5555"})
        )["userErrors"],
        json!([{"field": ["phone"], "message": "Phone has already been taken"}])
    );
    assert_eq!(
        create(&mut shop, json!({"firstName": "Bo", "phone": "call me"}))["userErrors"],
        json!([{"field": ["phone"], "message": "Phone is invalid"}])
    );
    assert_eq!(
        create(
            &mut shop,
            json!({"email": null, "phone": null, "firstName": null, "lastName": null})
        )["userErrors"],
        json!([{"field": null, "message": "Customer must have a name, phone number or email address"}])
    );
    assert_eq!(
        create(
            &mut shop,
            json!({"firstName": "Bo", "smsMarketingConsent": {"marketingState": "SUBSCRIBED"}})
        )["userErrors"],
        json!([{"field": ["smsMarketingConsent"], "message": "A phone number is required to set the SMS consent state."}])
    );
    assert_eq!(shop.count("Customer"), before);
    assert!(shop.events().is_empty());

    // An update changes what it is given; tags and consent changes have their topics.
    let updated = run(
        &mut shop,
        &format!(
            "mutation($input: CustomerInput!) {{ customerUpdate(input: $input) {{
                customer {{ {CUSTOMER_FIELDS} }} userErrors {{ field message }} }} }}"
        ),
        json!({"input": {"id": id, "lastName": "Smith", "tags": ["vip", "returning"],
            "emailMarketingConsent": {"marketingState": "UNSUBSCRIBED"},
            "metafields": [{"namespace": "custom", "key": "tier", "value": "silver"}]}}),
    );
    let steve = &data(&updated, "customerUpdate")["customer"];
    assert_eq!(steve["displayName"], json!("Steve Smith"));
    assert_eq!(steve["firstName"], json!("Steve"));
    assert_eq!(steve["tags"], json!(["vip", "returning"]));
    assert_eq!(
        steve["emailMarketingConsent"]["marketingState"],
        json!("UNSUBSCRIBED")
    );
    assert_eq!(steve["metafield"]["value"], json!("silver"));
    assert_eq!(
        topics(&mut shop),
        [
            "customers/update",
            "customer.tags_added",
            "customer.tags_removed",
            "customers_email_marketing_consent/update"
        ]
    );
    let unknown = run(
        &mut shop,
        "mutation { customerUpdate(input: {id: \"gid://shopify/Customer/1\", firstName: \"Tobi\"}) { customer { id } userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        data(&unknown, "customerUpdate"),
        &json!({"customer": null, "userErrors": [{"field": ["id"], "message": "Customer does not exist"}]})
    );

    // A customer with orders stays; one without goes.
    let jane = customer(&shop, "jane.doe@example.com");
    let refused = run(
        &mut shop,
        "mutation($id: ID!) { customerDelete(input: {id: $id}) { deletedCustomerId userErrors { field message } } }",
        json!({"id": jane}),
    );
    assert_eq!(
        data(&refused, "customerDelete")["userErrors"],
        json!([{"field": ["id"], "message": "Customer can\u{2019}t be deleted because they have associated orders"}])
    );
    assert!(shop.contains(&jane));
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { customerDelete(input: {id: $id}) { deletedCustomerId shop { id } userErrors { field message } } }",
        json!({"id": id}),
    );
    let payload = data(&deleted, "customerDelete");
    assert_eq!(payload["deletedCustomerId"], json!(id));
    assert!(payload["shop"]["id"].is_string());
    assert!(!shop.contains(&id));
    let events = shop.take_events();
    assert_eq!(events[0].topic, "customers/delete");
    assert_eq!(
        events[0].snapshot.as_ref().unwrap()["email"],
        json!("steve@example.com")
    );
}

#[test]
fn customer_set_creates_updates_and_upserts() {
    let (_, mut shop) = demo();
    let set = |shop: &mut ShopState, identifier: Value, input: Value| {
        let answer = run(
            shop,
            "mutation($identifier: CustomerSetIdentifiers, $input: CustomerSetInput!) {
                customerSet(identifier: $identifier, input: $input) {
                    customer { id email firstName phone } userErrors { field message code } } }",
            json!({"identifier": identifier, "input": input}),
        );
        data(&answer, "customerSet").clone()
    };
    let created = set(
        &mut shop,
        Value::Null,
        json!({"email": "new@example.com", "firstName": "New"}),
    );
    assert_eq!(created["userErrors"], json!([]));
    let id = created["customer"]["id"].clone();
    assert_eq!(topics(&mut shop), ["customers/create"]);

    // By email: the customer of that email is updated.
    let jane = customer(&shop, "jane.doe@example.com");
    let upserted = set(
        &mut shop,
        json!({"email": "Jane.Doe@example.com"}),
        json!({"firstName": "Janet"}),
    );
    assert_eq!(upserted["customer"]["id"], json!(jane));
    assert_eq!(upserted["customer"]["firstName"], json!("Janet"));
    assert_eq!(upserted["customer"]["email"], json!("jane.doe@example.com"));
    assert_eq!(topics(&mut shop), ["customers/update"]);
    // By an email no customer has: created with it.
    let fresh = set(
        &mut shop,
        json!({"email": "fresh@example.com"}),
        json!({"lastName": "Fresh"}),
    );
    assert_eq!(fresh["customer"]["email"], json!("fresh@example.com"));
    assert_ne!(fresh["customer"]["id"], id);
    // By phone, whatever its separators: Jane's is `+1 555 010 9988` in the data.
    let by_phone = set(
        &mut shop,
        json!({"phone": "+15550109988"}),
        json!({"lastName": "D."}),
    );
    assert_eq!(by_phone["customer"]["id"], json!(jane));
    // By id.
    let by_id = set(
        &mut shop,
        json!({"id": id}),
        json!({"phone": "+33612345678"}),
    );
    assert_eq!(by_id["customer"]["phone"], json!("+33612345678"));
    // By custom id: created with the metafield, found by it after.
    let custom = json!({"customId": {"namespace": "custom", "key": "erp_id", "value": "C-42"}});
    let first = set(
        &mut shop,
        custom.clone(),
        json!({"email": "erp@example.com"}),
    );
    let again = set(&mut shop, custom, json!({"firstName": "Erp"}));
    assert_eq!(first["customer"]["id"], again["customer"]["id"]);
    shop.take_events();

    let before = shop.count("Customer");
    assert_eq!(
        set(
            &mut shop,
            json!({"id": "gid://shopify/Customer/1"}),
            json!({"firstName": "X"})
        )["userErrors"][0]["code"],
        json!("NOT_FOUND")
    );
    assert_eq!(
        set(
            &mut shop,
            json!({"email": "a@example.com"}),
            json!({"email": "b@example.com"})
        )["userErrors"][0]["code"],
        json!("INPUT_MISMATCH")
    );
    assert_eq!(
        set(&mut shop, Value::Null, json!({"email": "new@example.com"}))["userErrors"],
        json!([{"field": ["email"], "message": "Email has already been taken", "code": "TAKEN"}])
    );
    assert_eq!(shop.count("Customer"), before);
    assert!(shop.events().is_empty());
}

#[test]
fn the_address_book_of_a_customer() {
    let (_, mut shop) = demo();
    let noah = customer(&shop, "new.customer@example.com");
    let create = |shop: &mut ShopState, address: Value, default: bool| {
        let answer = run(
            shop,
            "mutation($id: ID!, $address: MailingAddressInput!, $default: Boolean) {
                customerAddressCreate(customerId: $id, address: $address, setAsDefault: $default) {
                    address { id address1 country countryCodeV2 name } userErrors { field message } } }",
            json!({"id": noah, "address": address, "default": default}),
        );
        data(&answer, "customerAddressCreate").clone()
    };
    let home = create(
        &mut shop,
        json!({"address1": "1 Rue de Rivoli", "city": "Paris", "countryCode": "FR", "firstName": "Noah", "lastName": "Nguyen"}),
        false,
    );
    assert_eq!(home["userErrors"], json!([]));
    assert_eq!(home["address"]["country"], json!("France"));
    assert_eq!(home["address"]["name"], json!("Noah Nguyen"));
    let home_id = home["address"]["id"].as_str().unwrap().to_string();
    // The first address is the default one; another is not, unless asked.
    let office = create(&mut shop, json!({"address1": "2 Main St"}), false);
    let office_id = office["address"]["id"].as_str().unwrap().to_string();
    let default_of = |shop: &mut ShopState| {
        let answer = run(
            shop,
            "query($id: ID!) { customer(id: $id) { defaultAddress { id } addressesV2(first: 5) { nodes { id } } } }",
            json!({"id": noah}),
        );
        answer["data"]["customer"].clone()
    };
    let book = default_of(&mut shop);
    assert_eq!(book["defaultAddress"]["id"], json!(home_id));
    assert_eq!(book["addressesV2"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(topics(&mut shop), ["customers/update", "customers/update"]);

    let updated = run(
        &mut shop,
        "mutation($customer: ID!, $address: ID!) {
            customerAddressUpdate(customerId: $customer, addressId: $address, address: {city: \"Lyon\", zip: \"69001\"}, setAsDefault: true) {
                address { id address1 city zip } userErrors { field message } } }",
        json!({"customer": noah, "address": office_id}),
    );
    assert_eq!(
        data(&updated, "customerAddressUpdate")["address"],
        json!({"id": office_id, "address1": "2 Main St", "city": "Lyon", "zip": "69001"})
    );
    assert_eq!(
        default_of(&mut shop)["defaultAddress"]["id"],
        json!(office_id)
    );

    let switched = run(
        &mut shop,
        "mutation($customer: ID!, $address: ID!) { customerUpdateDefaultAddress(customerId: $customer, addressId: $address) {
            customer { defaultAddress { id } } userErrors { field message } } }",
        json!({"customer": noah, "address": home_id}),
    );
    assert_eq!(
        data(&switched, "customerUpdateDefaultAddress")["customer"]["defaultAddress"]["id"],
        json!(home_id)
    );
    shop.take_events();

    // The default address cannot go; another can.
    let delete = |shop: &mut ShopState, address: &str| {
        let answer = run(
            shop,
            "mutation($customer: ID!, $address: ID!) { customerAddressDelete(customerId: $customer, addressId: $address) {
                deletedAddressId userErrors { field message } } }",
            json!({"customer": noah, "address": address}),
        );
        data(&answer, "customerAddressDelete").clone()
    };
    assert_eq!(
        delete(&mut shop, &home_id)["userErrors"],
        json!([{"field": ["addressId"], "message": "Cannot delete the customer\u{2019}s default address"}])
    );
    assert!(shop.events().is_empty());
    assert_eq!(
        delete(&mut shop, &office_id)["deletedAddressId"],
        json!(office_id)
    );
    assert_eq!(topics(&mut shop), ["customers/update"]);
    assert_eq!(
        delete(
            &mut shop,
            "gid://shopify/MailingAddress/999999?model_name=CustomerAddress"
        )["userErrors"],
        json!([{"field": ["addressId"], "message": "Address does not exist"}])
    );
}

#[test]
fn marketing_consent() {
    let (_, mut shop) = demo();
    let jane = customer(&shop, "jane.doe@example.com");
    let noah = customer(&shop, "new.customer@example.com");
    let now = shop.timestamp();
    let email = run(
        &mut shop,
        "mutation($input: CustomerEmailMarketingConsentUpdateInput!) { customerEmailMarketingConsentUpdate(input: $input) {
            customer { emailMarketingConsent { marketingState marketingOptInLevel consentUpdatedAt } defaultEmailAddress { marketingState } }
            userErrors { field message code } } }",
        json!({"input": {"customerId": jane, "emailMarketingConsent": {"marketingState": "UNSUBSCRIBED", "consentUpdatedAt": "2026-01-07T15:50:00Z"}}}),
    );
    assert_eq!(
        data(&email, "customerEmailMarketingConsentUpdate"),
        &json!({"customer": {
            "emailMarketingConsent": {"marketingState": "UNSUBSCRIBED", "marketingOptInLevel": "SINGLE_OPT_IN", "consentUpdatedAt": "2026-01-07T15:50:00Z"},
            "defaultEmailAddress": {"marketingState": "UNSUBSCRIBED"},
        }, "userErrors": []})
    );
    assert_eq!(
        topics(&mut shop),
        [
            "customers/update",
            "customers_email_marketing_consent/update"
        ]
    );
    let sms = run(
        &mut shop,
        "mutation($input: CustomerSmsMarketingConsentUpdateInput!) { customerSmsMarketingConsentUpdate(input: $input) {
            customer { smsMarketingConsent { marketingState marketingOptInLevel consentUpdatedAt consentCollectedFrom } }
            userErrors { field message code } } }",
        json!({"input": {"customerId": jane, "smsMarketingConsent": {"marketingState": "SUBSCRIBED", "marketingOptInLevel": "SINGLE_OPT_IN"}}}),
    );
    assert_eq!(
        data(&sms, "customerSmsMarketingConsentUpdate")["customer"]["smsMarketingConsent"],
        json!({"marketingState": "SUBSCRIBED", "marketingOptInLevel": "SINGLE_OPT_IN", "consentUpdatedAt": now, "consentCollectedFrom": "OTHER"})
    );
    assert_eq!(
        topics(&mut shop),
        ["customers/update", "customers_marketing_consent/update"]
    );

    // Noah has no phone; a consent mutation does not set NOT_SUBSCRIBED.
    let refused = run(
        &mut shop,
        "mutation($input: CustomerSmsMarketingConsentUpdateInput!) { customerSmsMarketingConsentUpdate(input: $input) {
            customer { id } userErrors { field message code } } }",
        json!({"input": {"customerId": noah, "smsMarketingConsent": {"marketingState": "SUBSCRIBED"}}}),
    );
    assert_eq!(
        data(&refused, "customerSmsMarketingConsentUpdate")["userErrors"],
        json!([{"field": ["input", "customerId"], "message": "A phone number is required to set the SMS consent state.", "code": "MISSING_ARGUMENT"}])
    );
    let not_allowed = run(
        &mut shop,
        "mutation($input: CustomerEmailMarketingConsentUpdateInput!) { customerEmailMarketingConsentUpdate(input: $input) {
            customer { id } userErrors { field code } } }",
        json!({"input": {"customerId": noah, "emailMarketingConsent": {"marketingState": "NOT_SUBSCRIBED"}}}),
    );
    assert_eq!(
        data(&not_allowed, "customerEmailMarketingConsentUpdate")["userErrors"],
        json!([{"field": ["input", "emailMarketingConsent", "marketingState"], "code": "INCLUSION"}])
    );
    assert!(shop.events().is_empty());
}

#[test]
fn account_invitations_activation_urls_and_tax_exemptions() {
    let (_, mut shop) = demo();
    let jane = customer(&shop, "jane.doe@example.com");
    let created = run(
        &mut shop,
        "mutation { customerCreate(input: {email: \"invitee@example.com\"}) { customer { id } } }",
        Value::Null,
    );
    let invitee = data(&created, "customerCreate")["customer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let url = |shop: &mut ShopState, id: &str| {
        let answer = run(
            shop,
            "mutation($id: ID!) { customerGenerateAccountActivationUrl(customerId: $id) { accountActivationUrl userErrors { field message } } }",
            json!({"id": id}),
        );
        data(&answer, "customerGenerateAccountActivationUrl").clone()
    };
    let activation = url(&mut shop, &invitee);
    let link = activation["accountActivationUrl"].as_str().unwrap();
    let number = crate::state::parse_gid(&invitee).unwrap().1;
    assert!(
        link.starts_with(&format!(
            "https://local-supply-co.myshopify.com/account/activate/{number}/"
        )),
        "{link}"
    );
    assert!(shop.events().is_empty());
    assert_eq!(
        url(&mut shop, &jane)["userErrors"],
        json!([{"field": ["customerId"], "message": "Account already enabled"}])
    );

    let invited = run(
        &mut shop,
        "mutation($id: ID!) { customerSendAccountInviteEmail(customerId: $id) { customer { state } userErrors { field message code } } }",
        json!({"id": invitee}),
    );
    assert_eq!(
        data(&invited, "customerSendAccountInviteEmail")["customer"]["state"],
        json!("INVITED")
    );
    assert!(
        invited["extensions"]["lsf"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("not sent")
    );
    assert_eq!(topics(&mut shop), ["customers/update"]);

    let exempt = run(
        &mut shop,
        "mutation($id: ID!) { customerAddTaxExemptions(customerId: $id, taxExemptions: [CA_STATUS_CARD_EXEMPTION]) { customer { taxExemptions } userErrors { message } } }",
        json!({"id": invitee}),
    );
    assert_eq!(
        data(&exempt, "customerAddTaxExemptions")["customer"]["taxExemptions"],
        json!(["CA_STATUS_CARD_EXEMPTION"])
    );
    let removed = run(
        &mut shop,
        "mutation($id: ID!) { customerRemoveTaxExemptions(customerId: $id, taxExemptions: [CA_STATUS_CARD_EXEMPTION]) { customer { taxExemptions } } }",
        json!({"id": invitee}),
    );
    assert_eq!(
        data(&removed, "customerRemoveTaxExemptions")["customer"]["taxExemptions"],
        json!([])
    );
}

#[test]
fn customers_are_searched_and_sorted_by_their_documented_filters() {
    let (_, mut shop) = demo();
    let emails = |shop: &mut ShopState, query: &str| -> Vec<String> {
        let answer = run(
            shop,
            "query($query: String) { customers(first: 50, query: $query) { nodes { email } } }",
            json!({"query": query}),
        );
        answer["data"]["customers"]["nodes"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"))
            .iter()
            .map(|node| node["email"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        emails(&mut shop, "accepts_marketing:true"),
        ["jane.doe@example.com"]
    );
    assert_eq!(emails(&mut shop, "tag:vip"), ["jane.doe@example.com"]);
    assert_eq!(
        emails(&mut shop, "orders_count:>1"),
        ["jane.doe@example.com"]
    );
    let spent = emails(&mut shop, "total_spent:>0");
    assert!(spent.contains(&"camille.martin@example.com".to_string()));
    assert!(!spent.contains(&"new.customer@example.com".to_string()));
    assert_eq!(
        emails(&mut shop, "email:alex.morgan@example.com"),
        ["alex.morgan@example.com"]
    );
    assert_eq!(emails(&mut shop, "Nguyen"), ["new.customer@example.com"]);
    assert!(emails(&mut shop, "last_abandoned_order_date:>2020-01-01").is_empty());

    let sorted = run(
        &mut shop,
        "{ customers(first: 10, sortKey: NAME) { nodes { displayName } } customersCount(query: \"accepts_marketing:false\") { count } }",
        Value::Null,
    );
    let names: Vec<&str> = sorted["data"]["customers"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["displayName"].as_str().unwrap())
        .collect();
    let mut expected = names.clone();
    expected.sort_by_key(|name| name.to_lowercase());
    assert_eq!(names, expected);
    // `customersCount` documents `id` alone: another filter is ignored, with Shopify's warning.
    assert_eq!(
        sorted["data"]["customersCount"]["count"],
        json!(names.len())
    );
    assert_eq!(
        sorted["extensions"]["search"][0]["warnings"][0]["field"],
        json!("accepts_marketing")
    );
    let by_email = run(
        &mut shop,
        "{ customerByIdentifier(identifier: {emailAddress: \"Camille.Martin@example.com\"}) { firstName } }",
        Value::Null,
    );
    assert_eq!(
        by_email["data"]["customerByIdentifier"]["firstName"],
        json!("Camille")
    );
}

const COMPANY_FIELDS: &str = "id name externalId note
    mainContact { id isMainContact customer { email firstName } }
    contacts(first: 5) { nodes { id title roleAssignments(first: 5) { nodes { role { name } companyLocation { name } } } } }
    contactRoles(first: 5) { nodes { name } } defaultRole { name }
    locations(first: 5) { nodes { id name currency taxSettings { taxRegistrationId taxExempt }
        shippingAddress { id address1 city country countryCode zoneCode recipient companyName }
        billingAddress { id } } }";

#[test]
fn company_create_with_its_contact_and_location() {
    let (_, mut shop) = demo();
    let created = run(
        &mut shop,
        &format!(
            "mutation($input: CompanyCreateInput!) {{ companyCreate(input: $input) {{
                company {{ {COMPANY_FIELDS} }} userErrors {{ field message code }} }} }}"
        ),
        json!({"input": {
            "company": {"name": "Postal Cards Inc", "externalId": "01456606-0001"},
            "companyLocation": {"name": "Ottawa Postal Cards", "billingSameAsShipping": true, "taxRegistrationId": "CA-123",
                "shippingAddress": {"firstName": "Avery", "lastName": "Brown", "address1": "150 Elgin Street", "city": "Ottawa", "zoneCode": "ON", "zip": "K2P 1L4", "countryCode": "CA"}},
            "companyContact": {"email": "avery.brown@example.com", "firstName": "Avery", "lastName": "Brown", "title": "Buyer"},
        }}),
    );
    let payload = data(&created, "companyCreate");
    assert_eq!(payload["userErrors"], json!([]));
    let company = &payload["company"];
    assert_eq!(company["name"], json!("Postal Cards Inc"));
    assert_eq!(
        company["mainContact"]["customer"],
        json!({"email": "avery.brown@example.com", "firstName": "Avery"})
    );
    assert_eq!(company["mainContact"]["isMainContact"], json!(true));
    assert_eq!(
        company["contactRoles"]["nodes"],
        json!([{"name": "Location admin"}, {"name": "Ordering only"}])
    );
    assert_eq!(company["defaultRole"]["name"], json!("Ordering only"));
    let contact = &company["contacts"]["nodes"][0];
    assert_eq!(contact["title"], json!("Buyer"));
    assert_eq!(
        contact["roleAssignments"]["nodes"],
        json!([{"role": {"name": "Ordering only"}, "companyLocation": {"name": "Ottawa Postal Cards"}}])
    );
    let location = &company["locations"]["nodes"][0];
    assert_eq!(
        location["taxSettings"]["taxRegistrationId"],
        json!("CA-123")
    );
    let shipping = &location["shippingAddress"];
    assert_eq!(shipping["country"], json!("Canada"));
    assert_eq!(shipping["recipient"], json!("Avery Brown"));
    assert_eq!(shipping["companyName"], json!("Postal Cards Inc"));
    assert_eq!(location["billingAddress"]["id"], shipping["id"]);
    // The contact's id is its customer's, as the checkout's.
    let customer = customer(&shop, "avery.brown@example.com");
    assert_eq!(
        crate::state::parse_gid(contact["id"].as_str().unwrap())
            .unwrap()
            .1,
        crate::state::parse_gid(&customer).unwrap().1
    );
    assert_eq!(
        topics(&mut shop),
        [
            "companies/create",
            "company_locations/create",
            "customers/create",
            "company_contacts/create",
            "company_contact_roles/assign"
        ]
    );

    // With its name alone: a location of the same name, no address, no contact.
    let minimal = run(
        &mut shop,
        "mutation { companyCreate(input: {company: {name: \"Solo\"}}) { company { mainContact { id } locations(first: 2) { nodes { name shippingAddress { id } } } } } }",
        Value::Null,
    );
    assert_eq!(
        data(&minimal, "companyCreate")["company"],
        json!({"mainContact": null, "locations": {"nodes": [{"name": "Solo", "shippingAddress": null}]}})
    );
    shop.take_events();

    // shopify.dev's example of an error, and a contact whose email is taken.
    let companies = shop.count("Company");
    let nameless = run(
        &mut shop,
        "mutation { companyCreate(input: {company: {externalId: \"123\"}}) { company { id } userErrors { field message code } } }",
        Value::Null,
    );
    assert_eq!(
        data(&nameless, "companyCreate"),
        &json!({"company": null, "userErrors": [{"field": ["input", "company", "name"], "message": "Name must exist", "code": "REQUIRED"}]})
    );
    let taken = run(
        &mut shop,
        "mutation { companyCreate(input: {company: {name: \"Dup\"}, companyContact: {email: \"jane.doe@example.com\"}}) { company { id } userErrors { field message code } } }",
        Value::Null,
    );
    assert_eq!(
        data(&taken, "companyCreate")["userErrors"],
        json!([{"field": ["input", "companyContact", "email"], "message": "Email has already been taken", "code": "TAKEN"}])
    );
    assert_eq!(shop.count("Company"), companies);
    assert!(shop.events().is_empty());
}

#[test]
fn companies_and_locations_are_updated_and_deleted() {
    let (_, mut shop) = demo();
    let northwind = shop.keys("Company").next().unwrap().to_string();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { companyUpdate(companyId: $id, input: {name: \"Northwind Inns\", note: \"Key account\"}) {
            company { name note locations(first: 1) { nodes { shippingAddress { companyName } } } } userErrors { message } } }",
        json!({"id": northwind}),
    );
    assert_eq!(
        data(&updated, "companyUpdate")["company"],
        json!({"name": "Northwind Inns", "note": "Key account", "locations": {"nodes": [{"shippingAddress": {"companyName": "Northwind Inns"}}]}})
    );
    assert_eq!(topics(&mut shop), ["companies/update"]);

    let created = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationCreate(companyId: $id, input: {name: \"Northwind Boise\", phone: \"+12085550100\", locale: \"en\",
            buyerExperienceConfiguration: {checkoutToDraft: true}}) {
            companyLocation { id name phone company { name } buyerExperienceConfiguration { checkoutToDraft payNowOnly } } userErrors { message } } }",
        json!({"id": northwind}),
    );
    let location = data(&created, "companyLocationCreate")["companyLocation"].clone();
    assert_eq!(location["company"]["name"], json!("Northwind Inns"));
    assert_eq!(
        location["buyerExperienceConfiguration"],
        json!({"checkoutToDraft": true, "payNowOnly": false})
    );
    let id = location["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["company_locations/create"]);

    let assigned = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationAssignAddress(locationId: $id, addressTypes: [BILLING, SHIPPING],
            address: {address1: \"1 Main St\", city: \"Boise\", zoneCode: \"ID\", countryCode: US}) {
            addresses { id country } userErrors { field message code } } }",
        json!({"id": id}),
    );
    let addresses = &data(&assigned, "companyLocationAssignAddress")["addresses"];
    assert_eq!(addresses[0]["country"], json!("United States"));
    assert_eq!(addresses[0]["id"], addresses[1]["id"]);
    let taxes = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationTaxSettingsUpdate(companyLocationId: $id, taxRegistrationId: \"US-9\", taxExempt: true,
            exemptionsToAssign: [US_ID_RESELLER_EXEMPTION]) {
            companyLocation { taxSettings { taxRegistrationId taxExempt taxExemptions } } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&taxes, "companyLocationTaxSettingsUpdate")["companyLocation"]["taxSettings"],
        json!({"taxRegistrationId": "US-9", "taxExempt": true, "taxExemptions": ["US_ID_RESELLER_EXEMPTION"]})
    );
    assert_eq!(
        topics(&mut shop),
        ["company_locations/update", "company_locations/update"]
    );
    let renamed = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationUpdate(companyLocationId: $id, input: {name: \"Northwind Idaho\"}) { companyLocation { name } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&renamed, "companyLocationUpdate")["companyLocation"]["name"],
        json!("Northwind Idaho")
    );
    shop.take_events();

    // Errors change nothing.
    let unknown = run(
        &mut shop,
        "mutation { companyLocationCreate(companyId: \"gid://shopify/Company/1\", input: {name: \"X\"}) { companyLocation { id } userErrors { field message code } } }",
        Value::Null,
    );
    assert_eq!(
        data(&unknown, "companyLocationCreate")["userErrors"],
        json!([{"field": ["companyId"], "message": "Resource requested does not exist.", "code": "RESOURCE_NOT_FOUND"}])
    );
    let no_country = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationAssignAddress(locationId: $id, addressTypes: [SHIPPING], address: {address1: \"x\"}) { addresses { id } userErrors { field code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&no_country, "companyLocationAssignAddress")["userErrors"],
        json!([{"field": ["address", "countryCode"], "code": "BLANK"}])
    );
    assert!(shop.events().is_empty());

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { companyLocationDelete(companyLocationId: $id) { deletedCompanyLocationId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&deleted, "companyLocationDelete")["deletedCompanyLocationId"],
        json!(id)
    );
    assert!(!shop.contains(&id));
    assert_eq!(topics(&mut shop), ["company_locations/delete"]);

    // Deleting the company deletes its contacts and locations; its customers stay.
    let alex = customer(&shop, "alex.morgan@example.com");
    let gone = run(
        &mut shop,
        "mutation($id: ID!) { companyDelete(id: $id) { deletedCompanyId userErrors { message } } }",
        json!({"id": northwind}),
    );
    assert_eq!(
        data(&gone, "companyDelete")["deletedCompanyId"],
        json!(northwind)
    );
    assert_eq!(shop.count("Company"), 0);
    assert_eq!(shop.count("CompanyLocation"), 0);
    assert_eq!(shop.count("CompanyContact"), 0);
    assert_eq!(shop.count("CompanyContactRoleAssignment"), 0);
    assert_eq!(
        shop.get(&alex).unwrap()["companyContactProfiles"],
        json!([])
    );
    let events = shop.take_events();
    let topics: Vec<&str> = events.iter().map(|event| event.topic.as_str()).collect();
    assert_eq!(
        topics,
        [
            "company_contacts/delete",
            "company_locations/delete",
            "company_locations/delete",
            "companies/delete"
        ]
    );
    // The company is gone when the webhooks are built: the deletions embed it.
    assert_eq!(
        events[0].snapshot.as_ref().unwrap()["company"]["name"],
        json!("Northwind Inns")
    );
}

#[test]
fn contacts_main_contact_and_roles() {
    let (_, mut shop) = demo();
    let northwind = shop.keys("Company").next().unwrap().to_string();
    let noah = customer(&shop, "new.customer@example.com");
    let record = shop.get(&northwind).unwrap().clone();
    let portland = record["locations"][0].as_str().unwrap().to_string();
    let roles: Vec<String> = record["contactRoles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role.as_str().unwrap().to_string())
        .collect();

    let assigned = run(
        &mut shop,
        "mutation($company: ID!, $customer: ID!) { companyAssignCustomerAsContact(companyId: $company, customerId: $customer) {
            companyContact { id isMainContact customer { email } company { name } } userErrors { field message code } } }",
        json!({"company": northwind, "customer": noah}),
    );
    let contact = data(&assigned, "companyAssignCustomerAsContact")["companyContact"].clone();
    assert_eq!(
        contact["customer"]["email"],
        json!("new.customer@example.com")
    );
    let contact_id = contact["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["company_contacts/create"]);
    let again = run(
        &mut shop,
        "mutation($company: ID!, $customer: ID!) { companyAssignCustomerAsContact(companyId: $company, customerId: $customer) {
            companyContact { id } userErrors { field code } } }",
        json!({"company": northwind, "customer": noah}),
    );
    assert_eq!(
        data(&again, "companyAssignCustomerAsContact")["userErrors"],
        json!([{"field": ["customerId"], "code": "TAKEN"}])
    );

    let role = run(
        &mut shop,
        "mutation($contact: ID!, $role: ID!, $location: ID!) { companyContactAssignRole(companyContactId: $contact, companyContactRoleId: $role, companyLocationId: $location) {
            companyContactRoleAssignment { id role { name } companyLocation { id } companyContact { id } } userErrors { field message code } } }",
        json!({"contact": contact_id, "role": roles[0], "location": portland}),
    );
    let assignment =
        data(&role, "companyContactAssignRole")["companyContactRoleAssignment"].clone();
    assert_eq!(assignment["role"]["name"], json!("Location admin"));
    assert_eq!(assignment["companyLocation"]["id"], json!(portland));
    assert_eq!(topics(&mut shop), ["company_contact_roles/assign"]);
    let twice = run(
        &mut shop,
        "mutation($contact: ID!, $roles: [CompanyContactRoleAssign!]!) { companyContactAssignRoles(companyContactId: $contact, rolesToAssign: $roles) {
            roleAssignments { id } userErrors { field code } } }",
        json!({"contact": contact_id, "roles": [{"companyContactRoleId": roles[1], "companyLocationId": portland}]}),
    );
    assert_eq!(
        data(&twice, "companyContactAssignRoles")["userErrors"],
        json!([{"field": ["rolesToAssign", "0", "companyLocationId"], "code": "TAKEN"}])
    );
    let foreign = run(
        &mut shop,
        "mutation($contact: ID!, $location: ID!) { companyContactAssignRole(companyContactId: $contact, companyContactRoleId: \"gid://shopify/CompanyContactRole/1\", companyLocationId: $location) {
            companyContactRoleAssignment { id } userErrors { field code } } }",
        json!({"contact": contact_id, "location": portland}),
    );
    assert_eq!(
        data(&foreign, "companyContactAssignRole")["userErrors"],
        json!([{"field": ["companyContactRoleId"], "code": "RESOURCE_NOT_FOUND"}])
    );
    assert!(shop.events().is_empty());

    let main = run(
        &mut shop,
        "mutation($company: ID!, $contact: ID!) { companyAssignMainContact(companyId: $company, companyContactId: $contact) {
            company { mainContact { id } } userErrors { message } } }",
        json!({"company": northwind, "contact": contact_id}),
    );
    assert_eq!(
        data(&main, "companyAssignMainContact")["company"]["mainContact"]["id"],
        json!(contact_id)
    );
    assert_eq!(topics(&mut shop), ["companies/update"]);

    let revoked = run(
        &mut shop,
        "mutation($contact: ID!, $assignment: ID!) { companyContactRevokeRole(companyContactId: $contact, companyContactRoleAssignmentId: $assignment) {
            revokedCompanyContactRoleAssignmentId userErrors { message } } }",
        json!({"contact": contact_id, "assignment": assignment["id"]}),
    );
    assert_eq!(
        data(&revoked, "companyContactRevokeRole")["revokedCompanyContactRoleAssignmentId"],
        assignment["id"]
    );
    let events = shop.take_events();
    assert_eq!(events[0].topic, "company_contact_roles/revoke");
    assert_eq!(
        events[0].snapshot.as_ref().unwrap()["companyLocation"],
        json!(portland)
    );

    let updated = run(
        &mut shop,
        "mutation($contact: ID!) { companyContactUpdate(companyContactId: $contact, input: {title: \"Buyer\", lastName: \"Ng\"}) {
            companyContact { title customer { lastName } } userErrors { message } } }",
        json!({"contact": contact_id}),
    );
    assert_eq!(
        data(&updated, "companyContactUpdate")["companyContact"],
        json!({"title": "Buyer", "customer": {"lastName": "Ng"}})
    );
    assert_eq!(
        topics(&mut shop),
        ["company_contacts/update", "customers/update"]
    );

    let deleted = run(
        &mut shop,
        "mutation($contact: ID!) { companyContactDelete(companyContactId: $contact) { deletedCompanyContactId userErrors { message } } }",
        json!({"contact": contact_id}),
    );
    assert_eq!(
        data(&deleted, "companyContactDelete")["deletedCompanyContactId"],
        json!(contact_id)
    );
    assert!(shop.contains(&noah));
    assert_eq!(shop.get(&northwind).unwrap()["mainContact"], Value::Null);
    assert_eq!(topics(&mut shop), ["company_contacts/delete"]);

    let created = run(
        &mut shop,
        "mutation($company: ID!) { companyContactCreate(companyId: $company, input: {email: \"buyer@northwind.example\", firstName: \"Bea\"}) {
            companyContact { customer { email state } } userErrors { message } } }",
        json!({"company": northwind}),
    );
    assert_eq!(
        data(&created, "companyContactCreate")["companyContact"]["customer"],
        json!({"email": "buyer@northwind.example", "state": "DISABLED"})
    );
    assert_eq!(
        topics(&mut shop),
        ["customers/create", "company_contacts/create"]
    );
}

#[test]
fn what_the_backend_creates_syncs_to_the_store() {
    let (base, mut shop) = demo();
    let northwind = shop.keys("Company").next().unwrap().to_string();
    let created = run(
        &mut shop,
        "mutation { customerCreate(input: {email: \"lena@example.com\", firstName: \"Lena\", tags: [\"b2b\"],
            emailMarketingConsent: {marketingState: SUBSCRIBED}}) { customer { id } } }",
        Value::Null,
    );
    let lena = data(&created, "customerCreate")["customer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    run(
        &mut shop,
        "mutation($id: ID!) { customerAddressCreate(customerId: $id, address: {address1: \"9 Quai\", city: \"Lyon\", countryCode: FR, zip: \"69002\"}) { address { id } } }",
        json!({"id": lena}),
    );
    let contact = run(
        &mut shop,
        "mutation($company: ID!, $customer: ID!) { companyAssignCustomerAsContact(companyId: $company, customerId: $customer) { companyContact { id } } }",
        json!({"company": northwind, "customer": lena}),
    );
    let contact = data(&contact, "companyAssignCustomerAsContact")["companyContact"]["id"].clone();
    let record = shop.get(&northwind).unwrap().clone();
    let seattle = record["locations"][1].as_str().unwrap().to_string();
    let role = record["defaultRole"].as_str().unwrap().to_string();
    run(
        &mut shop,
        "mutation($contact: ID!, $role: ID!, $location: ID!) { companyContactAssignRole(companyContactId: $contact, companyContactRoleId: $role, companyLocationId: $location) { companyContactRoleAssignment { id } } }",
        json!({"contact": contact, "role": role, "location": seattle}),
    );

    let overlay = crate::sync::overlay(&shop, &base).expect("something to sync");
    let (store, diagnostics) = lsf_core::store::load::load_with_overlays(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
        std::slice::from_ref(&overlay),
    );
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    // The checkout logs in by email: Lena is there, with her address and her company.
    let customer = store
        .customer_named("lena@example.com")
        .unwrap()
        .expect("the customer created");
    assert_eq!(crate::state::gid("Customer", customer.id), lena);
    assert!(customer.accepts_marketing);
    assert_eq!(customer.tags, ["b2b"]);
    assert_eq!(customer.addresses[0].city, "Lyon");
    let company = &store.companies[customer.company.expect("a company")];
    assert_eq!(company.name, "Northwind Hotels");
    let locations: Vec<&str> = customer
        .company_locations
        .iter()
        .map(|index| company.locations[*index].name.as_str())
        .collect();
    assert_eq!(locations, ["Northwind Seattle"]);
}
