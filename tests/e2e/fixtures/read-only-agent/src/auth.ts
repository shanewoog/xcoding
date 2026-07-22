export function authorize(token: string | undefined): boolean {
  return token === "fixture-token";
}

export const authModuleName = "fixture authentication";