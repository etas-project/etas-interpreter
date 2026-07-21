use etas_effects::HostRequirementKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostServiceAvailability {
    pub host_authority: bool,
    pub model: bool,
    pub tool: bool,
    pub memory: bool,
    pub session: bool,
    pub filesystem: bool,
    pub command: bool,
    pub network: bool,
    pub tcp: bool,
    pub stream: bool,
    pub tls: bool,
    pub browser: bool,
    pub approval: bool,
    pub policy: bool,
    pub checkpoint: bool,
    pub time: bool,
    pub secret_access: bool,
    pub console: bool,
}

impl HostServiceAvailability {
    pub fn with_host(kind: HostRequirementKind) -> Self {
        let mut availability = Self::default();
        availability.enable(kind);
        availability
    }

    pub fn enable(&mut self, kind: HostRequirementKind) {
        match kind {
            HostRequirementKind::Agentic => self.model = true,
            HostRequirementKind::ToolCall => self.tool = true,
            HostRequirementKind::HostAuthority => {
                self.host_authority = true;
                self.filesystem = true;
                self.command = true;
                self.network = true;
                self.tcp = true;
                self.stream = true;
                self.tls = true;
                self.browser = true;
            }
            HostRequirementKind::DurableMemory => self.memory = true,
            HostRequirementKind::Approval => self.approval = true,
            HostRequirementKind::Checkpoint => self.checkpoint = true,
            HostRequirementKind::Time => self.time = true,
            HostRequirementKind::Network => {
                self.network = true;
                self.tcp = true;
                self.stream = true;
                self.tls = true;
            }
            HostRequirementKind::Tcp => self.tcp = true,
            HostRequirementKind::Stream => self.stream = true,
            HostRequirementKind::Tls => self.tls = true,
            HostRequirementKind::Browser => self.browser = true,
            HostRequirementKind::Console => self.console = true,
            HostRequirementKind::FileIO => self.filesystem = true,
            HostRequirementKind::Command => self.command = true,
            HostRequirementKind::SecretAccess => self.secret_access = true,
        }
    }

    pub fn supports(self, kind: HostRequirementKind) -> bool {
        match kind {
            HostRequirementKind::Agentic => self.model,
            HostRequirementKind::ToolCall => self.tool,
            HostRequirementKind::HostAuthority => self.host_authority,
            HostRequirementKind::DurableMemory => self.memory,
            HostRequirementKind::Approval => self.approval,
            HostRequirementKind::Checkpoint => self.checkpoint,
            HostRequirementKind::Time => self.time,
            HostRequirementKind::Network => self.network,
            HostRequirementKind::Tcp => self.tcp,
            HostRequirementKind::Stream => self.stream,
            HostRequirementKind::Tls => self.tls,
            HostRequirementKind::Browser => self.browser,
            HostRequirementKind::Console => self.console,
            HostRequirementKind::FileIO => self.filesystem,
            HostRequirementKind::Command => self.command,
            HostRequirementKind::SecretAccess => self.secret_access,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substrate_availability_is_granular() {
        let cli_like = HostServiceAvailability {
            tcp: true,
            stream: true,
            tls: true,
            browser: false,
            ..HostServiceAvailability::default()
        };

        assert!(cli_like.supports(HostRequirementKind::Tcp));
        assert!(cli_like.supports(HostRequirementKind::Stream));
        assert!(cli_like.supports(HostRequirementKind::Tls));
        assert!(
            !cli_like.supports(HostRequirementKind::Network),
            "a set of substrate adapters must not impersonate a broad network adapter"
        );
        assert!(
            !cli_like.supports(HostRequirementKind::Browser),
            "network/TCP/TLS support must not imply browser protocol support"
        );

        let network = HostServiceAvailability::with_host(HostRequirementKind::Network);
        assert!(network.supports(HostRequirementKind::Network));
        assert!(
            !network.supports(HostRequirementKind::Browser),
            "enabling broad network readiness must not create a browser adapter"
        );

        let tls_only = HostServiceAvailability {
            tls: true,
            ..HostServiceAvailability::default()
        };
        assert!(!tls_only.supports(HostRequirementKind::Network));

        let filesystem_only = HostServiceAvailability {
            filesystem: true,
            ..HostServiceAvailability::default()
        };
        assert!(!filesystem_only.supports(HostRequirementKind::HostAuthority));

        let all_individual = HostServiceAvailability {
            filesystem: true,
            command: true,
            network: true,
            tcp: true,
            stream: true,
            tls: true,
            browser: true,
            ..HostServiceAvailability::default()
        };
        assert!(
            !all_individual.supports(HostRequirementKind::HostAuthority),
            "individual adapters must not fabricate the distinct broad authority capability"
        );
    }
}
