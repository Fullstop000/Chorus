struct MyStruct;
mod a {
    impl super::MyStruct {
        pub fn foo() {}
    }
}
fn main() {
    MyStruct::foo();
}
