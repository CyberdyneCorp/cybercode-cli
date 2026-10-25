//! Standalone hub contract, also merged into the local API inventory. All component
//! names and references are namespaced before merging to avoid Task/Message collisions.
use crate::{api::*, error::Error, model::*};
use schemars::{
    JsonSchema,
    generate::{SchemaGenerator, SchemaSettings},
};
use serde_json::{Map, Value, json};

struct Contract {
    generator: SchemaGenerator,
    paths: Map<String, Value>,
}
impl Contract {
    fn add<I: JsonSchema, O: JsonSchema>(
        &mut self,
        method: &str,
        path: &str,
        id: &str,
        summary: &str,
        body: bool,
        query: &[&str],
    ) {
        let response = serde_json::to_value(self.generator.subschema_for::<O>())
            .expect("schema serialization");
        let error = serde_json::to_value(self.generator.subschema_for::<Error>())
            .expect("schema serialization");
        let mut parameters = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
            .map(|name| json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
            .collect::<Vec<_>>();
        parameters.extend(query.iter().map(
            |name| json!({"name":name,"in":"query","required":false,"schema":{"type":"string"}}),
        ));
        let mut responses = json!({"200":{"description":"Durable hub result","content":{"application/json":{"schema":response}}}});
        for (status, description) in [
            (400, "Invalid payload or repository"),
            (401, "Bearer authentication required"),
            (403, "Current identity, membership, role or step-up denied"),
            (404, "Project/resource not visible"),
            (409, "Revision, claim or operation conflict"),
            (429, "Rate limited"),
            (503, "Storage or live authorization unavailable"),
        ] {
            responses[status.to_string()] =
                json!({"description":description,"content":{"application/json":{"schema":error}}});
        }
        let mut operation = json!({"operationId":format!("v1.hub.{id}"),"tags":["hub"],"summary":summary,"parameters":parameters,"responses":responses,"security":[{"hubBearer":[]}],"x-cyber-listener":"hub"});
        if body {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":self.generator.subschema_for::<I>()}}});
        }
        if id == "hello" {
            operation["security"] = json!([]);
        }
        self.paths
            .entry(format!("/api/v1/hub{path}"))
            .or_insert_with(|| json!({}))[method] = operation;
    }
    fn read<O: JsonSchema>(&mut self, path: &str, id: &str, summary: &str, query: &[&str]) {
        self.add::<Empty, Data<O>>("get", path, id, summary, false, query);
    }
    fn write<I: JsonSchema, O: JsonSchema>(
        &mut self,
        method: &str,
        path: &str,
        id: &str,
        summary: &str,
    ) {
        self.add::<Mutation<I>, MutationResult<O>>(method, path, id, summary, true, &[]);
    }
}

pub fn document() -> Value {
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas".into();
    let mut c = Contract {
        generator: settings.into_generator(),
        paths: Map::new(),
    };
    c.read::<HubHello>(
        "/hello",
        "hello",
        "Pinned hub identity, OIDC issuer and resource audience",
        &[],
    );
    c.read::<Vec<Project>>(
        "/projects",
        "projects",
        "Projects visible to this verified account",
        &[],
    );
    c.write::<CreateProject, Project>(
        "post",
        "/projects",
        "createProject",
        "Create a repository-bound project as the configured bootstrap subject",
    );
    c.read::<Project>(
        "/projects/{projectID}",
        "project",
        "Project descriptor after current membership authorization",
        &[],
    );
    c.write::<UpdatePolicy, Project>(
        "patch",
        "/projects/{projectID}/policy",
        "policy",
        "Owner updates enrollment and guest policy at an expected revision",
    );
    c.read::<ContributionAuthorization>(
        "/projects/{projectID}/contributions/authorization",
        "contributionAuthorization",
        "Fresh human authorization for repository-bound contribution references",
        &["task", "agent"],
    );
    c.read::<Vec<Member>>(
        "/projects/{projectID}/members",
        "members",
        "Verified members, roles and guest attribution",
        &[],
    );
    c.write::<SetRole, Member>(
        "patch",
        "/projects/{projectID}/members/{memberID}",
        "memberRole",
        "Owner changes a role; invalidates old membership authority",
    );
    c.write::<Empty, Value>(
        "delete",
        "/projects/{projectID}/members/{memberID}",
        "removeMember",
        "Owner revokes membership and its agent capabilities",
    );
    c.write::<Empty, Value>(
        "post",
        "/projects/{projectID}/leave",
        "leave",
        "Leave a project and revoke own publications",
    );
    c.read::<Vec<InvitationView>>(
        "/projects/{projectID}/invitations",
        "invitations",
        "Owner invitation metadata; no invitation token",
        &[],
    );
    c.write::<Invite, IssuedInvitation>(
        "post",
        "/projects/{projectID}/invitations",
        "invite",
        "Owner issues a single-use invitation; token appears once only",
    );
    c.write::<Empty, Value>(
        "delete",
        "/projects/{projectID}/invitations/{invitationID}",
        "revokeInvitation",
        "Owner revokes an unredeemed invitation",
    );
    c.read::<Vec<ApplicationView>>(
        "/projects/{projectID}/applications",
        "applications",
        "Owner enrollment applications",
        &[],
    );
    c.write::<Empty, IssuedChallenge>(
        "post",
        "/projects/{projectID}/applications/{applicationID}/approve",
        "approveApplication",
        "Owner approves enrollment; a separate guest/friends code appears once only",
    );
    c.write::<Empty, Value>(
        "post",
        "/projects/{projectID}/applications/{applicationID}/reject",
        "rejectApplication",
        "Owner rejects enrollment",
    );
    c.write::<Join, JoinOutcome>(
        "post",
        "/join",
        "join",
        "Verified account requests enrollment without implicitly opening a private project",
    );
    c.read::<JoinOutcome>("/applications/{applicationID}", "application", "Only the applying verified subject can inspect their own pending enrollment and approved repository confirmation", &[]);
    c.write::<Redeem, JoinOutcome>(
        "post",
        "/applications/{applicationID}/redeem",
        "redeem",
        "Confirm repository and, when required, redeem a separate project admission code",
    );
    c.write::<RegisterRuntime, RuntimeRegistration>(
        "post",
        "/projects/{projectID}/runtimes",
        "registerRuntime",
        "Register a local runtime owned by the current human member",
    );
    c.write::<Heartbeat, RuntimeRegistration>(
        "post",
        "/projects/{projectID}/runtimes/{runtimeID}/heartbeat",
        "heartbeat",
        "Refresh original-bounded live identity evidence and presence",
    );
    c.write::<Empty, Value>(
        "delete",
        "/projects/{projectID}/runtimes/{runtimeID}",
        "unregisterRuntime",
        "Disconnect a human-owned runtime and revoke publications",
    );
    c.read::<Vec<Agent>>(
        "/projects/{projectID}/agents",
        "agents",
        "Published agents with verified human owners and observed presence",
        &[],
    );
    c.write::<Publish, Published>(
        "post",
        "/projects/{projectID}/agents",
        "publish",
        "Human explicitly publishes an agent; capability appears once only",
    );
    c.write::<Empty, Published>(
        "post",
        "/projects/{projectID}/agents/{agentID}/renew",
        "renew",
        "Human renews one active publication capability; secret appears once only",
    );
    c.write::<Empty, Value>(
        "delete",
        "/projects/{projectID}/agents/{agentID}",
        "unpublish",
        "Human revokes a publication",
    );
    c.read::<Vec<Message>>(
        "/projects/{projectID}/messages",
        "messages",
        "Project conversation and direct messages visible to this principal",
        &["after"],
    );
    c.write::<SendMessage, Message>(
        "post",
        "/projects/{projectID}/messages",
        "send",
        "Send attributed untrusted content; author derives from credential",
    );
    c.read::<Vec<Delivery>>(
        "/projects/{projectID}/deliveries",
        "deliveries",
        "Queued and held messages for one locally owned published agent",
        &["agent", "after"],
    );
    c.read::<DeliveryAuthorization>(
        "/projects/{projectID}/deliveries/{messageID}/{agentID}/authorization",
        "authorization",
        "Recheck current source and receiver authority before local release",
        &[],
    );
    c.write::<Acknowledge, Delivery>(
        "post",
        "/projects/{projectID}/deliveries/{messageID}/{agentID}/ack",
        "acknowledge",
        "Hash-bound delivery state; no execution or tool approval authority",
    );
    c.read::<Vec<Task>>(
        "/projects/{projectID}/tasks",
        "tasks",
        "Shared tasks with revisions, claim fences and attributed self-reports",
        &[],
    );
    c.write::<CreateTask, Task>(
        "post",
        "/projects/{projectID}/tasks",
        "createTask",
        "Create a shared task without executing anything remotely",
    );
    c.write::<ClaimTask, Task>(
        "post",
        "/projects/{projectID}/tasks/{taskID}/claim",
        "claimTask",
        "Atomically claim one unassigned task at its expected revision",
    );
    c.write::<UpdateTask, Task>(
        "patch",
        "/projects/{projectID}/tasks/{taskID}",
        "updateTask",
        "Assignee updates a task using revision and claim generation",
    );
    c.write::<ReconcileTask, Task>(
        "post",
        "/projects/{projectID}/tasks/{taskID}/reconcile",
        "reconcileTask",
        "Owner acknowledges unknown work before replacing an uncertain assignment",
    );
    c.read::<Snapshot>(
        "/projects/{projectID}/snapshot",
        "snapshot",
        "Current authorized project snapshot; does not replay local execution",
        &[],
    );
    c.read::<EventPage>(
        "/projects/{projectID}/events",
        "events",
        "Authorized durable event replay; resync snapshot after cursor expiry",
        &["after"],
    );
    let mut schemas = Map::new();
    for (name, mut schema) in c.generator.take_definitions(true) {
        namespace(&mut schema);
        schemas.insert(format!("Hub{name}"), schema);
    }
    let mut paths = Value::Object(c.paths);
    namespace(&mut paths);
    json!({"openapi":"3.1.0","info":{"title":"Cyber Code project hub","version":env!("CARGO_PKG_VERSION")},"security":[{"hubBearer":[]}],"components":{"securitySchemes":{"hubBearer":{"type":"http","scheme":"bearer","description":"Verified human access JWT or scoped ccap_ agent capability; never a local owner password"}},"schemas":schemas},"paths":paths})
}

fn namespace(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "$ref" {
                    if let Some(reference) = value
                        .as_str()
                        .and_then(|v| v.strip_prefix("#/components/schemas/"))
                    {
                        *value = json!(format!("#/components/schemas/Hub{reference}"));
                    }
                } else {
                    namespace(value)
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                namespace(value)
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_hub_types_and_references_are_namespaced_and_operations_keep_their_listener() {
        let doc = document();
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        assert!(schemas.contains_key("HubTask"));
        assert!(schemas.keys().all(|key| key.starts_with("Hub")));
        fn refs(value: &Value, schemas: &Map<String, Value>) {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        if key == "$ref" {
                            let name = value
                                .as_str()
                                .unwrap()
                                .strip_prefix("#/components/schemas/")
                                .unwrap();
                            assert!(
                                name.starts_with("Hub") && schemas.contains_key(name),
                                "{name}"
                            );
                        } else {
                            refs(value, schemas)
                        }
                    }
                }
                Value::Array(items) => {
                    for value in items {
                        refs(value, schemas)
                    }
                }
                _ => {}
            }
        }
        refs(&doc, schemas);
        for path in doc["paths"].as_object().unwrap().values() {
            for operation in path.as_object().unwrap().values() {
                assert_eq!(operation["x-cyber-listener"], "hub");
                if operation["operationId"] != "v1.hub.hello" {
                    assert_eq!(operation["security"], json!([{"hubBearer":[]}]))
                }
            }
        }
    }
}
