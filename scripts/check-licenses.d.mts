export const ALLOWED_LICENSES: string[];
export const VERIFIED_PACKAGE_LICENSES: Record<string, string>;
export function isLicenseAllowed(
  expression: string,
  allowed?: readonly string[],
): boolean;
export function checkPackages(
  licenseMap: Record<
    string,
    { name: string; versions?: string[]; [key: string]: unknown }[]
  >,
  policy?: {
    allowed?: readonly string[];
    verified?: Readonly<Record<string, string>>;
  },
): { name: string; license: string }[];
