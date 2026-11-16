enum VehicleState{
    P1,
    P2,
    P3,
    P4,
    P5,
    P6,
    P7
}

pub struct TransportOffice{

}
pub struct GroupCode{

}
pub struct VehicleCategory{

}

pub struct VehicleNumber{
    vehicle_state:VehicleState,
    transport_office: TransportOffice,
    group_code: GroupCode,
    vehicle_category: VehicleCategory,
    registration_number: u32
}

impl ToString for VehicleNumber{
    fn to_string(&self)->ToString{
        println!("{:?}{:?}{:?}{:?}{:?}", self.vehicle_state, self.transport_office, self.group_code, self.vehicle_category, self.registration_number)
    }
}