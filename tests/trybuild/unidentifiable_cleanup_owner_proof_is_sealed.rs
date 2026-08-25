use bridge_core::execution_policy::UnidentifiableCleanupOwnerProofV1;
use bridge_core::resource_flight::ResourceFlightIdV1;

fn main() {
    let resource_flight_id =
        ResourceFlightIdV1::parse(format!("resource-flight-{}", "1".repeat(64))).unwrap();
    let _ = UnidentifiableCleanupOwnerProofV1::default();
    let _ = UnidentifiableCleanupOwnerProofV1(resource_flight_id.clone());
    let _: UnidentifiableCleanupOwnerProofV1 = resource_flight_id.into();
}
