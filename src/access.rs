#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Actions{
    Read,
    Write,
    Update,
    Delete,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum RolesEx{
    Base(Roles)
}

pub fn get_permissions_for_user(){


}

pub fn get_permissions_for_roles(action:Actions){

    
}

pub async fn check_if_user_has_permissions(s_data:web::Data<Arc<ServerData>>, req: actix_web::HttpRequest, required_permissions: Vec<Permissions>) -> bool {
    let claims = req.extensions().get::<Claims>();
    let is_allowed = match claims {
        Some(claims) => {
            let user_role = &claims.role;
            let user_permissions = get_permissions_for_role(user_role);
            for permission in required_permissions {
                if !user_permissions.contains(&permission) {
                    false
                }
            }
            true
        }
        None => false,
    }

}

