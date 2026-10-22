//! The billing REST API under `/orgs/{org_id}`: one file per collection, each with its `routes()`.

use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;

use crate::auth::openapi::{Granted, SignedIn};
use crate::auth::rest::AppContext;

mod invoice_lines;
mod invoices;
mod payments;
mod taxes;

#[derive(OpenApi)]
#[openapi(
    info(title = "megh billing", description = "Taxes, invoices and payments."),
    tags(
        (name = "taxes", description = "Percentage taxes a line can carry."),
        (name = "invoices", description = "A request for payment from a customer org. Created as a draft and changed only while a draft."),
        (name = "invoiceLines", description = "The products and services billed on an invoice, with quantity, unit price, discount and taxes. Changed only while the invoice is a draft."),
        (name = "payments", description = "Money received for an open invoice, by cash, cheque, card or bank transfer; a refund is the `refunded` part of the payment."),
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
        .merge(invoices::routes())
        .merge(invoice_lines::routes())
        .merge(payments::routes())
        .merge(taxes::routes());
    Granted.modify(router.get_openapi_mut());
    router
}
