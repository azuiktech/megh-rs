//! The billing REST API under `/orgs/{org_id}`: one file per collection, each with its `routes()`.

use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;

use crate::auth::openapi::{Granted, SignedIn};
use crate::auth::rest::AppContext;

mod order_lines;
mod orders;
mod payment_terms;
mod series;
mod taxes;
mod vendors;

#[derive(OpenApi)]
#[openapi(
    info(title = "megh billing", description = "Taxes, payment terms, number series, vendors and orders; invoices and payments follow."),
    tags(
        (name = "taxes", description = "Percentage taxes a line can carry."),
        (name = "paymentTerms", description = "Days after the invoice date that payment is due."),
        (name = "series", description = "Named runs of document numbers: prefix, next number, zero padding."),
        (name = "orders", description = "What a client asked to buy (sales order) or the org asked a vendor for (purchase order). Created as a draft and changed only while a draft."),
        (name = "orderLines", description = "The products and services of an order, with quantity, unit price, discount and taxes. Changed only while the order is a draft."),
        (name = "vendors", description = "Whom the org buys from: contact, address, tax id, default payment term and currency."),
    ),
    modifiers(&SignedIn),
    security(("session" = [], "csrf" = [])),
)]
struct Billing;

/// The billing API's OpenAPI description: title and how requests are signed in.
pub fn openapi() -> utoipa::openapi::OpenApi {
    Billing::openapi()
}

/// All billing routes, to be nested under a prefix such as `/api/v1`, each documented with its grants.
pub fn router() -> OpenApiRouter<AppContext> {
    let mut router = OpenApiRouter::with_openapi(Billing::openapi())
        .merge(taxes::routes())
        .merge(orders::routes())
        .merge(order_lines::routes())
        .merge(payment_terms::routes())
        .merge(series::routes())
        .merge(vendors::routes());
    Granted.modify(router.get_openapi_mut());
    router
}
